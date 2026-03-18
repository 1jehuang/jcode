use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Debug, Clone)]
pub(super) enum PendingLogin {
    /// Waiting for user to paste Claude OAuth code (verifier needed for token exchange)
    Claude {
        verifier: String,
        redirect_uri: Option<String>,
    },
    /// Waiting for user to paste Claude OAuth code for a specific named account
    ClaudeAccount {
        verifier: String,
        label: String,
        redirect_uri: Option<String>,
    },
    /// Waiting for user to paste an OpenAI OAuth callback URL/query.
    OpenAi {
        verifier: String,
        expected_state: String,
        redirect_uri: String,
    },
    /// Waiting for user to paste an OpenAI OAuth callback URL/query for a named account.
    OpenAiAccount {
        verifier: String,
        label: String,
        expected_state: String,
        redirect_uri: String,
    },
    /// Waiting for user to paste a Gemini OAuth callback URL/query or auth code.
    Gemini {
        verifier: String,
        expected_state: Option<String>,
        redirect_uri: String,
    },
    /// Waiting for user to paste an API key for an OpenAI-compatible provider.
    ApiKeyProfile {
        provider: String,
        docs_url: String,
        env_file: String,
        key_name: String,
        default_model: Option<String>,
    },
    /// Waiting for user to paste a Cursor API key.
    CursorApiKey,
    /// GitHub Copilot device flow in progress (polling in background)
    Copilot,
    /// Interactive provider selection (user picks a number)
    ProviderSelection,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PendingAccountLabel {
    Anthropic,
    OpenAi,
}

impl App {
    pub(super) fn show_jcode_subscription_status(&mut self) {
        let configured_key = crate::subscription_catalog::configured_api_key().is_some();
        let configured_base = crate::subscription_catalog::configured_api_base()
            .unwrap_or_else(|| crate::subscription_catalog::DEFAULT_JCODE_API_BASE.to_string());
        let runtime_mode = crate::subscription_catalog::is_runtime_mode_enabled();

        let mut message = String::from("**Jcode Subscription Status**\n\n");
        message.push_str(&format!(
            "- Credentials: {}\n",
            if configured_key {
                "configured"
            } else {
                "not configured (`/login jcode`)"
            }
        ));
        message.push_str(&format!(
            "- Router base: `{}`{}\n",
            configured_base,
            if crate::subscription_catalog::has_router_base() {
                ""
            } else {
                " _(default placeholder)_"
            }
        ));
        message.push_str(&format!(
            "- Runtime mode: {}\n\n",
            if runtime_mode {
                "active for this session"
            } else {
                "inactive for this session"
            }
        ));

        message.push_str("**Catalog**\n\n");
        for model in crate::subscription_catalog::curated_models() {
            let default_suffix = if model.default_enabled {
                " _(default)_"
            } else {
                ""
            };
            message.push_str(&format!(
                "- **{}** — `{}`{}\n  - {}\n  - {}\n",
                model.display_name,
                model.id,
                default_suffix,
                crate::subscription_catalog::routing_policy_detail(model),
                model.note
            ));
        }

        message.push_str("\n**Planned tiers**\n\n");
        for tier in [
            crate::subscription_catalog::JcodeTier::Starter20,
            crate::subscription_catalog::JcodeTier::Pro100,
        ] {
            message.push_str(&format!(
                "- {} — ${}/mo retail, about ${:.2} usable inference budget\n",
                tier.display_name(),
                tier.retail_price_usd(),
                tier.usable_budget_usd()
            ));
        }

        message.push_str(
            "\nUsage/billing reporting is not live yet; this command is a scaffold for the curated jcode-managed subscription path.",
        );

        self.push_display_message(DisplayMessage::system(message));
    }

    pub(super) fn show_auth_status(&mut self) {
        let status = crate::auth::AuthStatus::check();
        let icon = |state: crate::auth::AuthState| match state {
            crate::auth::AuthState::Available => "✓",
            crate::auth::AuthState::Expired => "⚠ expired",
            crate::auth::AuthState::NotConfigured => "✗",
        };
        let providers = crate::provider_catalog::auth_status_login_providers();
        let mut message = String::from(
            "**Authentication Status:**\n\n| Provider | Status | Method |\n|----------|--------|--------|\n",
        );
        for provider in providers {
            message.push_str(&format!(
                "| {} | {} | {} |\n",
                provider.display_name,
                icon(status.state_for_provider(provider)),
                status.method_detail_for_provider(provider),
            ));
        }
        message.push_str(
            "\nUse `/login <provider>` to authenticate. `/login jcode` is for curated jcode subscription access; `/account` lists Anthropic + OpenAI accounts, and `/account openai` focuses on OpenAI accounts.",
        );
        self.push_display_message(DisplayMessage::system(message));
    }

    pub(super) fn show_interactive_login(&mut self) {
        use std::fmt::Write as _;

        let status = crate::auth::AuthStatus::check();
        let icon = |state: crate::auth::AuthState| match state {
            crate::auth::AuthState::Available => "✓",
            crate::auth::AuthState::Expired => "⚠",
            crate::auth::AuthState::NotConfigured => "✗",
        };
        let providers = crate::provider_catalog::tui_login_providers();
        let mut message = String::from(
            "**Login** - select a provider:\n\n| # | Provider | Auth | Status |\n|---|----------|------|--------|\n",
        );
        for (index, provider) in providers.iter().enumerate() {
            let state = status.state_for_provider(*provider);
            let _ = writeln!(
                &mut message,
                "| {} | {} | {} | {} |",
                index + 1,
                provider.display_name,
                provider.auth_kind.label(),
                icon(state)
            );
        }
        let _ = write!(
            &mut message,
            "\nType a number (1-{}) or provider name, or `/cancel` to cancel.",
            providers.len()
        );
        self.push_display_message(DisplayMessage::system(message));
        self.pending_login = Some(PendingLogin::ProviderSelection);
    }

    pub(super) fn start_login_provider(
        &mut self,
        provider: crate::provider_catalog::LoginProviderDescriptor,
    ) {
        match provider.target {
            crate::provider_catalog::LoginProviderTarget::Jcode => self.start_jcode_login(),
            crate::provider_catalog::LoginProviderTarget::Claude => self.start_claude_login(),
            crate::provider_catalog::LoginProviderTarget::OpenAi => self.start_openai_login(),
            crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                self.start_openrouter_login()
            }
            crate::provider_catalog::LoginProviderTarget::Azure => {
                self.push_display_message(DisplayMessage::error(
                    "Azure OpenAI login is currently CLI-only. Run `jcode login --provider azure`."
                        .to_string(),
                ));
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
                self.start_openai_compatible_profile_login(profile)
            }
            crate::provider_catalog::LoginProviderTarget::Cursor => self.start_cursor_login(),
            crate::provider_catalog::LoginProviderTarget::Copilot => self.start_copilot_login(),
            crate::provider_catalog::LoginProviderTarget::Gemini => self.start_gemini_login(),
            crate::provider_catalog::LoginProviderTarget::Antigravity => {
                self.start_antigravity_login()
            }
            crate::provider_catalog::LoginProviderTarget::Google => {
                self.push_display_message(DisplayMessage::error(
                    "Google/Gmail login is only available from the CLI right now. Run `jcode login --provider google`."
                        .to_string(),
                ));
            }
        }
    }

    fn start_claude_login(&mut self) {
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

        let listener = match crate::auth::oauth::bind_callback_listener(0) {
            Ok(l) => l,
            Err(_) => {
                self.start_claude_login_manual();
                return;
            }
        };
        let port = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(_) => {
                self.start_claude_login_manual();
                return;
            }
        };

        let redirect_uri = format!("http://localhost:{}/callback", port);

        let auth_url = crate::auth::oauth::claude_auth_url(&redirect_uri, &challenge, &verifier);
        let manual_auth_url = crate::auth::oauth::claude_auth_url(
            crate::auth::oauth::claude::REDIRECT_URI,
            &challenge,
            &verifier,
        );
        let qr_section = crate::login_qr::markdown_section(
            &manual_auth_url,
            "No browser on this machine? Scan this on another device, finish login there, then paste the full callback URL here:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             Waiting for callback on port {}...\n\
             If the browser didn't open, visit:\n{}\n\n\
             Or paste the authorization code here to complete manually.{}",
            port, auth_url, qr_section
        )));
        self.set_status_notice("Login: waiting for browser...");
        self.pending_login = Some(PendingLogin::Claude {
            verifier: verifier.clone(),
            redirect_uri: Some(redirect_uri.clone()),
        });

        let verifier_clone = verifier;
        let redirect_clone = redirect_uri;
        tokio::spawn(async move {
            match crate::auth::oauth::wait_for_callback_async_on_listener(listener, &verifier_clone)
                .await
            {
                Ok(code) => {
                    match Self::claude_token_exchange(
                        verifier_clone,
                        code,
                        "default",
                        Some(redirect_clone),
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: false,
                                message: format!("Claude login failed: {}", e),
                            }));
                        }
                    }
                }
                Err(e) => {
                    crate::logging::info(&format!(
                        "Callback server error (user may paste manually): {}",
                        e
                    ));
                }
            }
        });
    }

    fn start_jcode_login(&mut self) {
        self.push_display_message(DisplayMessage::system(format!(
            "**Jcode Subscription Login**\n\nPaste your jcode subscription API key. This is distinct from OpenRouter BYOK and is meant for curated jcode-managed access.\n\nCurated models: {}\n\nOptional: after the key, jcode can also store a custom router base URL if you have one.",
            crate::subscription_catalog::curated_models()
                .iter()
                .map(|model| model.display_name)
                .collect::<Vec<_>>()
                .join(", ")
        )));
        self.set_status_notice("Login: jcode API key...");
        self.pending_login = Some(PendingLogin::ApiKeyProfile {
            provider: "Jcode Subscription".to_string(),
            docs_url: "https://subscription.jcode.invalid".to_string(),
            env_file: crate::subscription_catalog::JCODE_ENV_FILE.to_string(),
            key_name: crate::subscription_catalog::JCODE_API_KEY_ENV.to_string(),
            default_model: Some(crate::subscription_catalog::default_model().id.to_string()),
        });
    }

    fn start_claude_login_manual(&mut self) {
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

        let auth_url = crate::auth::oauth::claude_auth_url(
            crate::auth::oauth::claude::REDIRECT_URI,
            &challenge,
            &verifier,
        );
        let qr_section = crate::login_qr::markdown_section(
            &auth_url,
            "Scan this on another device if this machine has no browser:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             After logging in, copy the callback URL or authorization code and **paste it here**.{}",
            auth_url, qr_section
        )));
        self.set_status_notice("Login: paste code...");
        self.pending_login = Some(PendingLogin::Claude {
            verifier,
            redirect_uri: None,
        });
    }

    pub(super) fn start_claude_login_for_account(&mut self, label: &str) {
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

        let auth_url = crate::auth::oauth::claude_auth_url(
            crate::auth::oauth::claude::REDIRECT_URI,
            &challenge,
            &verifier,
        );
        let qr_section = crate::login_qr::markdown_section(
            &auth_url,
            "Scan this on another device if this machine has no browser:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login** (account: `{}`)\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             After logging in, copy the callback URL or authorization code and **paste it here**.{}",
            label, auth_url, qr_section
        )));
        self.set_status_notice(&format!("Login [{}]: paste code...", label));
        self.pending_login = Some(PendingLogin::ClaudeAccount {
            verifier,
            label: label.to_string(),
            redirect_uri: None,
        });
    }

    pub(super) fn show_accounts(&mut self) {
        self.open_account_picker(false);
    }

    fn render_anthropic_accounts_markdown(&self) -> String {
        let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        let active_label = crate::auth::claude::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();

        if accounts.is_empty() {
            return "**Anthropic Accounts:** none configured\n\n\
                 Use `/account add <label>` to add an account, or `/login claude` for a default account."
                .to_string();
        }

        let mut lines = vec!["**Anthropic Accounts:**\n".to_string()];
        lines.push("| Account | Email | Status | Subscription | Active |".to_string());
        lines.push("|---------|-------|--------|-------------|--------|".to_string());

        for account in &accounts {
            let is_active = active_label.as_deref() == Some(&account.label);
            let status = if account.expires > now_ms {
                "✓ valid"
            } else {
                "⚠ expired"
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let sub = account.subscription_type.as_deref().unwrap_or("unknown");
            let active_mark = if is_active { "◉" } else { "" };
            lines.push(format!(
                "| {} | {} | {} | {} | {} |",
                account.label, email, status, sub, active_mark
            ));
        }

        lines.push(String::new());
        lines.push("Commands: `/account switch <label>`, `/account add <label>`, `/account remove <label>`".to_string());

        lines.join("\n")
    }

    pub(super) fn show_openai_accounts(&mut self) {
        self.open_account_picker(true);
    }

    pub(super) fn open_account_picker(&mut self, openai_only: bool) {
        use crate::tui::account_picker::{
            AccountPicker, AccountPickerItem, AccountPickerItemKind, AccountProviderKind,
        };

        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut items = Vec::new();

        if !openai_only {
            let active_label = crate::auth::claude::active_account_label();
            for account in crate::auth::claude::list_accounts().unwrap_or_default() {
                let label = account.label.clone();
                let status = if account.expires > now_ms {
                    "valid".to_string()
                } else {
                    "expired".to_string()
                };
                let detail = account
                    .subscription_type
                    .as_deref()
                    .map(|sub| format!("plan: {}", sub))
                    .unwrap_or_else(|| "plan: unknown".to_string());
                items.push(AccountPickerItem {
                    provider: AccountProviderKind::Anthropic,
                    kind: AccountPickerItemKind::Existing {
                        label: label.clone(),
                        masked_email: account
                            .email
                            .as_deref()
                            .map(mask_email)
                            .unwrap_or_else(|| "unknown".to_string()),
                        status,
                        detail,
                        is_active: active_label.as_deref() == Some(label.as_str()),
                    },
                });
            }
            items.push(AccountPickerItem {
                provider: AccountProviderKind::Anthropic,
                kind: AccountPickerItemKind::NewAccount,
            });
        }

        let active_label = crate::auth::codex::active_account_label();
        for account in crate::auth::codex::list_accounts().unwrap_or_default() {
            let label = account.label.clone();
            let status = match account.expires_at {
                Some(expires_at) if expires_at > now_ms => "valid".to_string(),
                Some(_) => "expired".to_string(),
                None => "valid".to_string(),
            };
            let detail = account
                .account_id
                .as_deref()
                .map(|id| format!("acct: {}", id))
                .unwrap_or_else(|| "acct: unknown".to_string());
            items.push(AccountPickerItem {
                provider: AccountProviderKind::OpenAi,
                kind: AccountPickerItemKind::Existing {
                    label: label.clone(),
                    masked_email: account
                        .email
                        .as_deref()
                        .map(mask_email)
                        .unwrap_or_else(|| "unknown".to_string()),
                    status,
                    detail,
                    is_active: active_label.as_deref() == Some(label.as_str()),
                },
            });
        }
        items.push(AccountPickerItem {
            provider: AccountProviderKind::OpenAi,
            kind: AccountPickerItemKind::NewAccount,
        });

        let title = if openai_only {
            " OpenAI Accounts "
        } else {
            " Accounts "
        };
        self.account_picker_overlay =
            Some(std::cell::RefCell::new(AccountPicker::new(title, items)));
    }

    pub(super) fn handle_account_picker_command(
        &mut self,
        command: crate::tui::account_picker::AccountPickerCommand,
    ) {
        use crate::tui::account_picker::{AccountPickerCommand, AccountProviderKind};

        match command {
            AccountPickerCommand::Switch { provider, label } => match provider {
                AccountProviderKind::Anthropic => self.switch_account(&label),
                AccountProviderKind::OpenAi => self.switch_openai_account(&label),
            },
            AccountPickerCommand::Login { provider, label } => match provider {
                AccountProviderKind::Anthropic => self.start_claude_login_for_account(&label),
                AccountProviderKind::OpenAi => self.start_openai_login_for_account(&label),
            },
            AccountPickerCommand::Remove { provider, label } => match provider {
                AccountProviderKind::Anthropic => self.remove_account(&label),
                AccountProviderKind::OpenAi => self.remove_openai_account(&label),
            },
            AccountPickerCommand::PromptNew { provider } => self.prompt_new_account_label(provider),
        }
    }

    pub(super) fn prompt_new_account_label(
        &mut self,
        provider: crate::tui::account_picker::AccountProviderKind,
    ) {
        let (display, pending) = match provider {
            crate::tui::account_picker::AccountProviderKind::Anthropic => {
                ("Anthropic", PendingAccountLabel::Anthropic)
            }
            crate::tui::account_picker::AccountProviderKind::OpenAi => {
                ("OpenAI", PendingAccountLabel::OpenAi)
            }
        };
        self.push_display_message(DisplayMessage::system(format!(
            "Enter a label for the new {} account, then press Enter. Use `/cancel` to abort.",
            display
        )));
        self.set_status_notice(&format!("Account: new {} label...", display));
        self.pending_account_label = Some(pending);
    }

    pub(super) fn handle_pending_account_label_input(
        &mut self,
        pending: PendingAccountLabel,
        input: String,
    ) {
        let trimmed = input.trim();
        if trimmed == "/cancel" {
            self.push_display_message(DisplayMessage::system(
                "New account creation cancelled.".to_string(),
            ));
            self.set_status_notice("Account: cancelled");
            return;
        }
        if trimmed.is_empty() {
            self.push_display_message(DisplayMessage::error(
                "Account label cannot be empty.".to_string(),
            ));
            self.pending_account_label = Some(pending);
            return;
        }

        match pending {
            PendingAccountLabel::Anthropic => self.start_claude_login_for_account(trimmed),
            PendingAccountLabel::OpenAi => self.start_openai_login_for_account(trimmed),
        }
    }

    pub(super) fn account_command_for_picker(
        command: &crate::tui::account_picker::AccountPickerCommand,
    ) -> Option<String> {
        use crate::tui::account_picker::{AccountPickerCommand, AccountProviderKind};

        match command {
            AccountPickerCommand::Switch { .. } => None,
            AccountPickerCommand::Login { provider, label } => Some(match provider {
                AccountProviderKind::Anthropic => format!("/account add {}", label),
                AccountProviderKind::OpenAi => format!("/account openai add {}", label),
            }),
            AccountPickerCommand::Remove { provider, label } => Some(match provider {
                AccountProviderKind::Anthropic => format!("/account remove {}", label),
                AccountProviderKind::OpenAi => format!("/account openai remove {}", label),
            }),
            AccountPickerCommand::PromptNew { .. } => None,
        }
    }

    pub(super) fn next_account_picker_action(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> anyhow::Result<Option<crate::tui::account_picker::AccountPickerCommand>> {
        use crate::tui::account_picker::OverlayAction;

        let action = {
            let Some(picker_cell) = self.account_picker_overlay.as_ref() else {
                return Ok(None);
            };
            let mut picker = picker_cell.borrow_mut();
            picker.handle_overlay_key(code, modifiers)?
        };

        match action {
            OverlayAction::Continue => Ok(None),
            OverlayAction::Close => {
                self.account_picker_overlay = None;
                Ok(None)
            }
            OverlayAction::Execute(command) => {
                self.account_picker_overlay = None;
                Ok(Some(command))
            }
        }
    }

    fn render_openai_accounts_markdown(&self) -> String {
        let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
        let active_label = crate::auth::codex::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();

        if accounts.is_empty() {
            return "**OpenAI Accounts:** none configured\n\n\
                 Use `/account openai add <label>` to add an account, or `/login openai` for a default account."
                .to_string();
        }

        let mut lines = vec!["**OpenAI Accounts:**\n".to_string()];
        lines.push("| Account | Email | Status | ChatGPT Account ID | Active |".to_string());
        lines.push("|---------|-------|--------|--------------------|--------|".to_string());

        for account in &accounts {
            let is_active = active_label.as_deref() == Some(&account.label);
            let status = match account.expires_at {
                Some(expires_at) if expires_at > now_ms => "✓ valid",
                Some(_) => "⚠ expired",
                None => "✓ valid",
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let account_id = account.account_id.as_deref().unwrap_or("unknown");
            let active_mark = if is_active { "◉" } else { "" };
            lines.push(format!(
                "| {} | {} | {} | {} | {} |",
                account.label, email, status, account_id, active_mark
            ));
        }

        lines.push(String::new());
        lines.push(
            "Commands: `/account openai switch <label>`, `/account openai add <label>`, `/account openai remove <label>`"
                .to_string(),
        );

        lines.join("\n")
    }

    pub(super) fn switch_account(&mut self, label: &str) {
        match crate::auth::claude::set_active_account(label) {
            Ok(()) => {
                {
                    let provider = self.provider.clone();
                    let label_owned = label.to_string();
                    tokio::spawn(async move {
                        provider.invalidate_credentials().await;
                        crate::logging::info(&format!(
                            "Switched to Anthropic account '{}'",
                            label_owned
                        ));
                    });
                }
                self.push_display_message(DisplayMessage::system(format!(
                    "Switched to Anthropic account `{}`.",
                    label
                )));
                // Keep account-sensitive UI state in sync immediately.
                crate::auth::AuthStatus::invalidate_cache();
                self.context_limit = self.provider.context_window() as u64;
                self.context_warning_shown = false;
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch account: {}",
                    e
                )));
            }
        }
    }

    pub(super) fn switch_account_by_label(&mut self, label: &str) {
        let has_anthropic = crate::auth::claude::list_accounts()
            .unwrap_or_default()
            .iter()
            .any(|account| account.label == label);
        let has_openai = crate::auth::codex::list_accounts()
            .unwrap_or_default()
            .iter()
            .any(|account| account.label == label);

        match (has_anthropic, has_openai) {
            (true, false) => self.switch_account(label),
            (false, true) => self.switch_openai_account(label),
            (true, true) => self.push_display_message(DisplayMessage::error(format!(
                "Account label `{}` exists for both Anthropic and OpenAI. Use `/account switch {}` or `/account openai switch {}` explicitly.",
                label, label, label
            ))),
            (false, false) => self.push_display_message(DisplayMessage::error(format!(
                "No Anthropic or OpenAI account with label `{}` found.",
                label
            ))),
        }
    }

    pub(super) fn remove_account(&mut self, label: &str) {
        match crate::auth::claude::remove_account(label) {
            Ok(()) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "Removed Anthropic account `{}`.",
                    label
                )));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to remove account: {}",
                    e
                )));
            }
        }
    }

    pub(super) fn switch_openai_account(&mut self, label: &str) {
        match crate::auth::codex::set_active_account(label) {
            Ok(()) => {
                {
                    let provider = self.provider.clone();
                    let label_owned = label.to_string();
                    tokio::spawn(async move {
                        provider.invalidate_credentials().await;
                        crate::logging::info(&format!(
                            "Switched to OpenAI account '{}'",
                            label_owned
                        ));
                    });
                }
                self.push_display_message(DisplayMessage::system(format!(
                    "Switched to OpenAI account `{}`.",
                    label
                )));
                crate::auth::AuthStatus::invalidate_cache();
                self.context_limit = self.provider.context_window() as u64;
                self.context_warning_shown = false;
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch OpenAI account: {}",
                    e
                )));
            }
        }
    }

    pub(super) fn remove_openai_account(&mut self, label: &str) {
        match crate::auth::codex::remove_account(label) {
            Ok(()) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "Removed OpenAI account `{}`.",
                    label
                )));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to remove OpenAI account: {}",
                    e
                )));
            }
        }
    }

    fn start_openai_login(&mut self) {
        self.start_openai_login_for_account("default");
    }

    fn start_openai_login_for_account(&mut self, label: &str) {
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

        let state: String = {
            let bytes: [u8; 16] = rand::random();
            hex::encode(bytes)
        };

        let port = crate::auth::oauth::openai::DEFAULT_PORT;
        let redirect_uri = crate::auth::oauth::openai::redirect_uri(port);
        let auth_url = crate::auth::oauth::openai_auth_url_with_prompt(
            &redirect_uri,
            &challenge,
            &state,
            Some("login"),
        );
        let qr_section = crate::login_qr::markdown_section(
            &auth_url,
            "Scan this on another device if this machine has no browser, then paste the full callback URL here:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let callback_listener = crate::auth::oauth::bind_callback_listener(port).ok();
        let callback_available = callback_listener.is_some();
        let browser_opened = open::that(&auth_url).is_ok();
        let label_owned = label.to_string();

        if let Some(listener) = callback_listener {
            let verifier_clone = verifier.clone();
            let state_clone = state.clone();
            let label_clone = label_owned.clone();
            tokio::spawn(async move {
                match Self::openai_login_callback(
                    verifier_clone,
                    state_clone,
                    Some(label_clone),
                    listener,
                )
                .await
                {
                    Ok(msg) => {
                        crate::logging::info(&format!("OpenAI login: {}", msg));
                        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                            provider: "openai".to_string(),
                            success: true,
                            message: msg,
                        }));
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "OpenAI automatic callback did not complete: {}",
                            e
                        ));
                    }
                }
            });
        }

        let callback_line = if callback_available {
            format!(
                "Waiting for callback on `localhost:{}`... (this will complete automatically)\n",
                port
            )
        } else {
            format!(
                "Local callback port `localhost:{}` is unavailable, so finish in any browser and paste the full callback URL here.\n",
                port
            )
        };
        let browser_line = if browser_opened {
            String::new()
        } else {
            "This machine could not open a browser automatically.\n".to_string()
        };

        self.push_display_message(DisplayMessage::system(format!(
            "**OpenAI OAuth Login** (account: `{}`)\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             **Note:** Wait a few seconds for the page to fully load before clicking Continue. \
             OpenAI's verification system may briefly disable the button.\n\n\
             {}{}\
             Or paste the full callback URL or query string here to finish from another device.{}",
            label, auth_url, browser_line, callback_line, qr_section
        )));
        self.set_status_notice(&format!("Login [{}]: waiting…", label));
        self.pending_login = if label == "default" {
            Some(PendingLogin::OpenAi {
                verifier,
                expected_state: state,
                redirect_uri,
            })
        } else {
            Some(PendingLogin::OpenAiAccount {
                verifier,
                label: label.to_string(),
                expected_state: state,
                redirect_uri,
            })
        };
    }

    async fn openai_login_callback(
        verifier: String,
        expected_state: String,
        label: Option<String>,
        listener: tokio::net::TcpListener,
    ) -> Result<String, String> {
        let port = crate::auth::oauth::openai::DEFAULT_PORT;
        let redirect_uri = crate::auth::oauth::openai::redirect_uri(port);
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            crate::auth::oauth::wait_for_callback_async_on_listener(listener, &expected_state),
        )
        .await
        .map_err(|_| "Login timed out after 5 minutes. Please try again.".to_string())?
        .map_err(|e| format!("Callback failed: {}", e))?;

        Self::openai_token_exchange(verifier, code, label, None, &redirect_uri).await
    }

    async fn openai_token_exchange(
        verifier: String,
        input: String,
        label: Option<String>,
        expected_state: Option<String>,
        redirect_uri: &str,
    ) -> Result<String, String> {
        let oauth_tokens = if let Some(expected_state) = expected_state {
            crate::auth::oauth::exchange_openai_callback_input(
                &verifier,
                input.trim(),
                &expected_state,
                redirect_uri,
            )
            .await
            .map_err(|e| e.to_string())?
        } else {
            crate::auth::oauth::exchange_openai_code(&input, &verifier, redirect_uri)
                .await
                .map_err(|e| e.to_string())?
        };

        let label = label.unwrap_or_else(|| "default".to_string());
        crate::auth::oauth::save_openai_tokens_for_account(&oauth_tokens, &label)
            .map_err(|e| format!("Failed to save tokens: {}", e))?;

        if label == "default" {
            Ok("Successfully logged in to OpenAI!".to_string())
        } else {
            Ok(format!(
                "Successfully logged in to OpenAI! (account: {})",
                label
            ))
        }
    }

    fn start_gemini_login(&mut self) {
        let (verifier, challenge) = crate::auth::oauth::generate_pkce_public();
        let state = crate::auth::oauth::generate_state_public();

        let callback_listener = crate::auth::oauth::bind_callback_listener(0).ok();
        let maybe_redirect_uri = callback_listener
            .as_ref()
            .and_then(|listener| listener.local_addr().ok())
            .map(|addr| format!("http://127.0.0.1:{}/oauth2callback", addr.port()));

        let auth_setup: anyhow::Result<(String, Option<String>, String)> =
            if let Some(redirect_uri) = maybe_redirect_uri {
                crate::auth::gemini::build_web_auth_url(&redirect_uri, &challenge, &state)
                    .map(|auth_url| (auth_url, Some(state.clone()), redirect_uri))
            } else {
                crate::auth::gemini::build_manual_auth_url(
                    "https://codeassist.google.com/authcode",
                    &challenge,
                    &state,
                )
                .map(|auth_url| {
                    (
                        auth_url,
                        None,
                        "https://codeassist.google.com/authcode".to_string(),
                    )
                })
            };

        let (auth_url, pending_state, redirect_uri) = match auth_setup {
            Ok(values) => values,
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Gemini login is unavailable: {}",
                    e
                )));
                self.set_status_notice("Login: failed");
                return;
            }
        };

        let qr_section = crate::login_qr::markdown_section(
            &auth_url,
            "Scan this on another device if this machine has no browser, then paste the callback URL or authorization code here:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let browser_opened = open::that(&auth_url).is_ok();
        let callback_available = callback_listener.is_some() && pending_state.is_some();

        if let (Some(listener), Some(expected_state)) = (callback_listener, pending_state.clone()) {
            let redirect_clone = redirect_uri.clone();
            tokio::spawn(async move {
                let code = tokio::time::timeout(
                    std::time::Duration::from_secs(300),
                    crate::auth::oauth::wait_for_callback_async_on_listener(
                        listener,
                        &expected_state,
                    ),
                )
                .await
                .map_err(|_| "Login timed out after 5 minutes. Please try again.".to_string())
                .and_then(|result| result.map_err(|e| format!("Callback failed: {}", e)));

                match code {
                    Ok(code) => {
                        match crate::auth::gemini::exchange_callback_code(&code, &redirect_clone)
                            .await
                        {
                            Ok(tokens) => {
                                let msg = if let Some(email) = tokens.email {
                                    format!(
                                        "Successfully logged in to Gemini! (account: {})",
                                        email
                                    )
                                } else {
                                    "Successfully logged in to Gemini!".to_string()
                                };
                                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                    provider: "gemini".to_string(),
                                    success: true,
                                    message: msg,
                                }));
                            }
                            Err(e) => {
                                let message = format!("Gemini login failed: {}", e);
                                crate::logging::info(&format!(
                                    "Gemini automatic callback did not complete: {}",
                                    e
                                ));
                                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                    provider: "gemini".to_string(),
                                    success: false,
                                    message,
                                }));
                            }
                        }
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Gemini automatic callback did not complete: {}",
                            e
                        ));
                        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                            provider: "gemini".to_string(),
                            success: false,
                            message: format!("Gemini login failed: {}", e),
                        }));
                    }
                }
            });
        }

        let callback_line = if callback_available {
            format!(
                "Waiting for callback on `{}`... (this will complete automatically)\n",
                redirect_uri
            )
        } else {
            "Finish login in any browser, then paste the callback URL or authorization code here.\n"
                .to_string()
        };
        let browser_line = if browser_opened {
            String::new()
        } else {
            "This machine could not open a browser automatically.\n".to_string()
        };

        self.push_display_message(DisplayMessage::system(format!(
            "**Gemini OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             {}{}\
             Or paste the full callback URL, query string, or authorization code here to finish.{}",
            auth_url, browser_line, callback_line, qr_section
        )));
        self.set_status_notice("Login: waiting…");
        self.pending_login = Some(PendingLogin::Gemini {
            verifier,
            expected_state: pending_state,
            redirect_uri,
        });
    }

    fn start_openrouter_login(&mut self) {
        self.start_api_key_login(
            "OpenRouter",
            "https://openrouter.ai/keys",
            "openrouter.env",
            "OPENROUTER_API_KEY",
            None,
        );
    }

    fn start_opencode_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::OPENCODE_PROFILE);
    }

    fn start_opencode_go_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::OPENCODE_GO_PROFILE);
    }

    fn start_zai_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::ZAI_PROFILE);
    }

    fn start_chutes_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::CHUTES_PROFILE);
    }

    fn start_cerebras_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::CEREBRAS_PROFILE);
    }

    fn start_openai_compatible_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::OPENAI_COMPAT_PROFILE);
    }

    fn start_openai_compatible_profile_login(
        &mut self,
        profile: crate::provider_catalog::OpenAiCompatibleProfile,
    ) {
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        self.start_api_key_login(
            &resolved.display_name,
            &resolved.setup_url,
            &resolved.env_file,
            &resolved.api_key_env,
            resolved.default_model.as_deref(),
        );
    }

    fn start_api_key_login(
        &mut self,
        provider: &str,
        docs_url: &str,
        env_file: &str,
        key_name: &str,
        default_model: Option<&str>,
    ) {
        let model_hint = default_model
            .map(|m| format!("Suggested default model: `{}`\n\n", m))
            .unwrap_or_default();
        self.push_display_message(DisplayMessage::system(format!(
            "**{} API Key**\n\n\
             Setup docs: {}\n\
             Stored variable: `{}`\n\
             {}\n\
             **Paste your API key below** (it will be saved securely).",
            provider, docs_url, key_name, model_hint
        )));
        self.set_status_notice("Login: paste key…");
        self.pending_login = Some(PendingLogin::ApiKeyProfile {
            provider: provider.to_string(),
            docs_url: docs_url.to_string(),
            env_file: env_file.to_string(),
            key_name: key_name.to_string(),
            default_model: default_model.map(|m| m.to_string()),
        });
    }

    fn start_cursor_login(&mut self) {
        let binary = crate::auth::cursor::cursor_agent_cli_path();

        if crate::auth::cursor::has_cursor_agent_cli() {
            self.push_display_message(DisplayMessage::system(format!(
                "**Cursor Login**\n\n\
                 Running `{} login` to open browser authentication.\n\n\
                 If that fails, jcode will fall back to saving a Cursor API key for `cursor-agent`.",
                binary
            )));
            self.set_status_notice("Login: cursor browser...");

            match Self::run_external_login_command(&binary, &["login"]) {
                Ok(()) => {
                    self.push_display_message(DisplayMessage::system(
                        "✓ **Cursor login completed.**".to_string(),
                    ));
                    self.set_status_notice("Login: ✓ cursor");
                    crate::auth::AuthStatus::invalidate_cache();
                    return;
                }
                Err(e) => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Cursor CLI login failed: {}\n\nFalling back to API key mode...",
                        e
                    )));
                }
            }
        }

        self.push_display_message(DisplayMessage::system(
            "**Cursor API Key**\n\n\
             Get your API key from: https://cursor.com/settings\n\
             (Dashboard > Integrations > User API Keys)\n\n\
             jcode will save it securely and provide it to `cursor-agent` at runtime.\n\
             You still need Cursor Agent installed to use the Cursor provider.\n\n\
             **Paste your API key below**."
                .to_string(),
        ));
        self.set_status_notice("Login: paste cursor key...");
        self.pending_login = Some(PendingLogin::CursorApiKey);
    }

    fn start_copilot_login(&mut self) {
        self.set_status_notice("Login: copilot device flow...");
        self.pending_login = Some(PendingLogin::Copilot);

        tokio::spawn(async move {
            let client = reqwest::Client::new();

            let device_resp = match crate::auth::copilot::initiate_device_flow(&client).await {
                Ok(resp) => resp,
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Copilot device flow failed: {}", e),
                    }));
                    return;
                }
            };

            let user_code = device_resp.user_code.clone();
            let verification_uri = device_resp.verification_uri.clone();

            let clipboard_ok = copy_to_clipboard(&user_code);
            let clipboard_msg = if clipboard_ok {
                " (copied to clipboard - just paste it!)"
            } else {
                ""
            };

            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                provider: "copilot_code".to_string(),
                success: true,
                message: {
                    let qr_section = crate::login_qr::markdown_section(
                        &verification_uri,
                        "Scan this on another device to open the GitHub verification page:",
                    )
                    .map(|section| format!("\n\n{section}"))
                    .unwrap_or_default();
                    format!(
                        "**GitHub Copilot Login**\n\n\
                         Your code: **{}**{}\n\n\
                         Opening browser to {} ...\n\
                         Paste the code there and authorize.{}\n\n\
                         Waiting for authorization... (type `/cancel` to abort)",
                        user_code, clipboard_msg, verification_uri, qr_section
                    )
                },
            }));

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = open::that_detached(&verification_uri);

            let token = match crate::auth::copilot::poll_for_access_token(
                &client,
                &device_resp.device_code,
                device_resp.interval,
            )
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Copilot login failed: {}", e),
                    }));
                    return;
                }
            };

            let username = crate::auth::copilot::fetch_github_username(&client, &token)
                .await
                .unwrap_or_else(|_| "unknown".to_string());

            match crate::auth::copilot::save_github_token(&token, &username) {
                Ok(()) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: true,
                        message: format!(
                            "✓ Authenticated as **{}** via GitHub Copilot.\n\n\
                             Copilot models are now available in `/model`.",
                            username
                        ),
                    }));
                }
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Failed to save Copilot token: {}", e),
                    }));
                }
            }
        });

        self.push_display_message(DisplayMessage::system(
            "**GitHub Copilot Login**\n\n\
             Starting device flow... please wait."
                .to_string(),
        ));
    }

    fn start_antigravity_login(&mut self) {
        let binary = std::env::var("JCODE_ANTIGRAVITY_CLI_PATH")
            .unwrap_or_else(|_| "antigravity".to_string());

        self.push_display_message(DisplayMessage::system(format!(
            "Starting Antigravity login...\n\nRunning `{} login`",
            binary
        )));
        self.set_status_notice("Login: antigravity...");

        match Self::run_external_login_command(&binary, &["login"]) {
            Ok(()) => {
                self.push_display_message(DisplayMessage::system(
                    "✓ **Antigravity login command completed.**".to_string(),
                ));
                self.set_status_notice("Login: ✓ antigravity");
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Antigravity login failed: {}\n\nCheck `{}` is installed and run `{} login`.",
                    e, binary, binary
                )));
                self.set_status_notice("Login: antigravity failed");
            }
        }
    }

    fn run_external_login_command(program: &str, args: &[&str]) -> anyhow::Result<()> {
        let raw_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }

        let status_result = std::process::Command::new(program).args(args).status();

        if raw_was_enabled {
            let _ = crossterm::terminal::enable_raw_mode();
        }

        let status = status_result.map_err(|e| {
            anyhow::anyhow!(
                "Failed to start command: {} {} ({})",
                program,
                args.join(" "),
                e
            )
        })?;
        if !status.success() {
            anyhow::bail!(
                "Command exited with non-zero status: {} {} ({})",
                program,
                args.join(" "),
                status
            );
        }
        Ok(())
    }

    fn run_external_login_command_owned(program: &str, args: &[String]) -> anyhow::Result<()> {
        let raw_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }

        let status_result = std::process::Command::new(program).args(args).status();

        if raw_was_enabled {
            let _ = crossterm::terminal::enable_raw_mode();
        }

        let status = status_result.map_err(|e| {
            anyhow::anyhow!(
                "Failed to start command: {} {} ({})",
                program,
                args.join(" "),
                e
            )
        })?;
        if !status.success() {
            anyhow::bail!(
                "Command exited with non-zero status: {} {} ({})",
                program,
                args.join(" "),
                status
            );
        }
        Ok(())
    }

    pub(super) fn handle_login_input(&mut self, pending: PendingLogin, input: String) {
        let trimmed = input.trim();
        if trimmed == "/cancel" {
            self.push_display_message(DisplayMessage::system("Login cancelled.".to_string()));
            return;
        }

        if trimmed.is_empty() {
            self.push_display_message(DisplayMessage::system(
                "Login still in progress. Complete it in your browser, or paste the callback URL / authorization code here. Type `/cancel` to abort.".to_string(),
            ));
            self.pending_login = Some(pending);
            return;
        }

        match &pending {
            PendingLogin::OpenAi { .. }
            | PendingLogin::OpenAiAccount { .. }
            | PendingLogin::Gemini {
                expected_state: Some(_),
                ..
            } if !looks_like_oauth_callback_input(trimmed) => {
                self.push_display_message(DisplayMessage::system(
                    "Still waiting for the browser callback. Paste the full callback URL or query string if you want to finish manually, or keep waiting for the automatic redirect.".to_string(),
                ));
                self.pending_login = Some(pending);
                return;
            }
            _ => {}
        }

        match pending {
            PendingLogin::Claude {
                verifier,
                redirect_uri,
            } => {
                self.set_status_notice("Login: exchanging...");
                let input_owned = input.clone();
                tokio::spawn(async move {
                    match Self::claude_token_exchange(
                        verifier,
                        input_owned,
                        "default",
                        redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: false,
                                message: format!("Claude login failed: {}", e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(
                    "Exchanging authorization code for tokens...".to_string(),
                ));
            }
            PendingLogin::ClaudeAccount {
                verifier,
                label,
                redirect_uri,
            } => {
                self.set_status_notice(&format!("Login [{}]: exchanging...", label));
                let input_owned = input.clone();
                let label_clone = label.clone();
                tokio::spawn(async move {
                    match Self::claude_token_exchange(
                        verifier,
                        input_owned,
                        &label_clone,
                        redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: false,
                                message: format!("Claude login [{}] failed: {}", label_clone, e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(format!(
                    "Exchanging authorization code for account `{}`...",
                    label
                )));
            }
            PendingLogin::OpenAi {
                verifier,
                expected_state,
                redirect_uri,
            } => {
                self.set_status_notice("Login: exchanging...");
                let input_owned = input.clone();
                tokio::spawn(async move {
                    match Self::openai_token_exchange(
                        verifier,
                        input_owned,
                        None,
                        Some(expected_state),
                        &redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "openai".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "openai".to_string(),
                                success: false,
                                message: format!("OpenAI login failed: {}", e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(
                    "Exchanging OpenAI callback for tokens...".to_string(),
                ));
            }
            PendingLogin::OpenAiAccount {
                verifier,
                label,
                expected_state,
                redirect_uri,
            } => {
                self.set_status_notice(&format!("Login [{}]: exchanging...", label));
                let input_owned = input.clone();
                let label_clone = label.clone();
                tokio::spawn(async move {
                    match Self::openai_token_exchange(
                        verifier,
                        input_owned,
                        Some(label_clone.clone()),
                        Some(expected_state),
                        &redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "openai".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "openai".to_string(),
                                success: false,
                                message: format!("OpenAI login [{}] failed: {}", label_clone, e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(format!(
                    "Exchanging OpenAI callback for account `{}`...",
                    label
                )));
            }
            PendingLogin::Gemini {
                verifier,
                expected_state,
                redirect_uri,
            } => {
                self.set_status_notice("Login: exchanging...");
                let input_owned = input.clone();
                tokio::spawn(async move {
                    match crate::auth::gemini::exchange_callback_input(
                        &verifier,
                        input_owned.trim(),
                        expected_state.as_deref(),
                        &redirect_uri,
                    )
                    .await
                    {
                        Ok(tokens) => {
                            let msg = if let Some(email) = tokens.email {
                                format!("Successfully logged in to Gemini! (account: {})", email)
                            } else {
                                "Successfully logged in to Gemini!".to_string()
                            };
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "gemini".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "gemini".to_string(),
                                success: false,
                                message: format!("Gemini login failed: {}", e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(
                    "Exchanging Gemini callback for tokens...".to_string(),
                ));
            }
            PendingLogin::ApiKeyProfile {
                provider,
                docs_url,
                env_file,
                key_name,
                default_model,
            } => {
                let key = input.trim().to_string();
                if key_name == "OPENROUTER_API_KEY" && !key.starts_with("sk-or-") {
                    self.push_display_message(DisplayMessage::system(
                        "⚠ OpenRouter keys typically start with `sk-or-`. Saving anyway..."
                            .to_string(),
                    ));
                }

                let save_result: anyhow::Result<()> =
                    if key_name == crate::subscription_catalog::JCODE_API_KEY_ENV {
                        (|| {
                            let mut content = format!("{}={}\n", key_name, key);
                            if let Some(base) = crate::subscription_catalog::configured_api_base() {
                                content.push_str(&format!(
                                    "{}={}\n",
                                    crate::subscription_catalog::JCODE_API_BASE_ENV,
                                    base
                                ));
                            }

                            let config_dir = dirs::config_dir()
                                .ok_or_else(|| anyhow::anyhow!("No config directory found"))?
                                .join("jcode");
                            std::fs::create_dir_all(&config_dir)?;
                            crate::platform::set_directory_permissions_owner_only(&config_dir)?;

                            let file_path = config_dir.join(&env_file);
                            std::fs::write(&file_path, content)?;
                            crate::platform::set_permissions_owner_only(&file_path)?;
                            crate::env::set_var(&key_name, &key);
                            Ok(())
                        })()
                    } else {
                        Self::save_named_api_key(&env_file, &key_name, &key)
                    };

                match save_result {
                    Ok(()) => {
                        let model_hint = default_model
                            .map(|m| format!("\nSuggested default model: `{}`", m))
                            .unwrap_or_default();
                        let guidance = if key_name == crate::subscription_catalog::JCODE_API_KEY_ENV
                        {
                            format!(
                                "Use `--provider jcode` or `/login jcode` to access curated models via your router.\nDocs: {}",
                                docs_url
                            )
                        } else if key_name == "OPENROUTER_API_KEY" {
                            "You can now use `/model` to switch to OpenRouter models.".to_string()
                        } else {
                            format!(
                                "Restart with `--provider {}` to use this backend in a new session.",
                                provider.to_lowercase().replace(' ', "-")
                            )
                        };
                        self.push_display_message(DisplayMessage::system(format!(
                            "✓ **{} API key saved!**\n\n\
                             Stored at `~/.config/jcode/{}`.\n\
                             {}{}",
                            provider, env_file, guidance, model_hint
                        )));
                        self.set_status_notice("Login: ✓ saved");
                    }
                    Err(e) => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to save {} key: {}",
                            provider, e
                        )));
                    }
                }
            }
            PendingLogin::CursorApiKey => {
                let key = input.trim().to_string();
                if key.is_empty() {
                    self.push_display_message(DisplayMessage::error(
                        "API key cannot be empty.".to_string(),
                    ));
                    self.pending_login = Some(PendingLogin::CursorApiKey);
                    return;
                }

                match crate::auth::cursor::save_api_key(&key) {
                    Ok(()) => {
                        crate::auth::AuthStatus::invalidate_cache();
                        self.push_display_message(DisplayMessage::system(
                            "✓ **Cursor API key saved!**\n\n\
                             Stored at `~/.config/jcode/cursor.env`.\n\
                             jcode will pass it to `cursor-agent` automatically.\n\
                             Install Cursor Agent if it is not already on PATH."
                                .to_string(),
                        ));
                        self.set_status_notice("Login: ✓ cursor");
                    }
                    Err(e) => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to save Cursor API key: {}",
                            e
                        )));
                    }
                }
            }
            PendingLogin::Copilot => {
                self.push_display_message(DisplayMessage::system(
                    "Copilot login is waiting for browser authorization.\n\
                     Complete the login in your browser, or type `/cancel` to abort."
                        .to_string(),
                ));
                self.pending_login = Some(PendingLogin::Copilot);
            }
            PendingLogin::ProviderSelection => {
                let providers = crate::provider_catalog::tui_login_providers();
                if let Some(provider) =
                    crate::provider_catalog::resolve_login_selection(&input, &providers)
                {
                    self.start_login_provider(provider);
                } else {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Unknown selection '{}'. Type 1-{} or a provider name.",
                        input.trim(),
                        providers.len()
                    )));
                    self.pending_login = Some(PendingLogin::ProviderSelection);
                }
            }
        }
    }

    pub(super) fn handle_login_completed(&mut self, login: LoginCompleted) {
        if login.provider == "copilot_code" {
            self.push_display_message(DisplayMessage::system(login.message.clone()));
            if let Some(code) = login
                .message
                .split("Enter code: **")
                .nth(1)
                .and_then(|s| s.split("**").next())
            {
                self.set_status_notice(&format!("Login: enter {} at GitHub", code));
            }
            return;
        }
        crate::auth::AuthStatus::invalidate_cache();
        if login.success {
            self.push_display_message(DisplayMessage::system(login.message));
            self.set_status_notice(&format!("Login: ✓ {}", login.provider));
            self.provider.on_auth_changed();
        } else {
            self.push_display_message(DisplayMessage::error(login.message));
            self.set_status_notice(&format!("Login: {} failed", login.provider));
        }
        if self.pending_login.is_some() {
            self.pending_login = None;
        }
    }

    pub(super) fn handle_update_status(&mut self, status: crate::bus::UpdateStatus) {
        use crate::bus::UpdateStatus;
        match status {
            UpdateStatus::Checking => {
                self.set_status_notice("Checking for updates...");
            }
            UpdateStatus::Available { current, latest } => {
                self.set_status_notice(&format!("Update available: {} → {}", current, latest));
            }
            UpdateStatus::Downloading { version } => {
                self.set_status_notice(&format!("⬇️  Downloading {}...", version));
            }
            UpdateStatus::Installed { version } => {
                self.set_status_notice(&format!("✅ Updated to {} — restarting", version));
            }
            UpdateStatus::UpToDate => {}
            UpdateStatus::Error(e) => {
                self.set_status_notice(&format!("Update failed: {}", e));
            }
        }
    }

    async fn claude_token_exchange(
        verifier: String,
        input: String,
        label: &str,
        redirect_uri: Option<String>,
    ) -> Result<String, String> {
        let fallback_redirect_uri =
            redirect_uri.unwrap_or_else(|| crate::auth::oauth::claude::REDIRECT_URI.to_string());
        let redirect_uri =
            crate::auth::oauth::claude_redirect_uri_for_input(input.trim(), &fallback_redirect_uri);
        let oauth_tokens =
            crate::auth::oauth::exchange_claude_code(&verifier, input.trim(), &redirect_uri)
                .await
                .map_err(|e| e.to_string())?;

        crate::auth::oauth::save_claude_tokens_for_account(&oauth_tokens, label)
            .map_err(|e| format!("Failed to save tokens: {}", e))?;

        let profile_suffix = match crate::auth::oauth::update_claude_account_profile(
            label,
            &oauth_tokens.access_token,
        )
        .await
        {
            Ok(Some(email)) => format!(" (email: {})", mask_email(&email)),
            Ok(None) => String::new(),
            Err(e) => {
                crate::logging::warn(&format!(
                    "Claude login [{}] profile fetch failed: {}",
                    label, e
                ));
                String::new()
            }
        };

        Ok(format!(
            "Successfully logged in to Claude! (account: {}){}",
            label, profile_suffix
        ))
    }

    fn save_named_api_key(env_file: &str, key_name: &str, key: &str) -> anyhow::Result<()> {
        if !crate::provider_catalog::is_safe_env_key_name(key_name) {
            anyhow::bail!("Invalid API key variable name: {}", key_name);
        }
        if !crate::provider_catalog::is_safe_env_file_name(env_file) {
            anyhow::bail!("Invalid env file name: {}", env_file);
        }

        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("No config directory found"))?
            .join("jcode");
        std::fs::create_dir_all(&config_dir)?;
        crate::platform::set_directory_permissions_owner_only(&config_dir)?;

        let file_path = config_dir.join(env_file);
        let content = format!("{}={}\n", key_name, key);
        std::fs::write(&file_path, &content)?;

        crate::platform::set_permissions_owner_only(&file_path)?;

        crate::env::set_var(key_name, key);
        Ok(())
    }
}

fn looks_like_oauth_callback_input(input: &str) -> bool {
    let input = input.trim();
    input.starts_with("http://")
        || input.starts_with("https://")
        || input.starts_with('?')
        || input.contains("code=")
        || input.contains("state=")
}

pub(super) fn handle_auth_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/auth" {
        app.show_auth_status();
        return true;
    }

    if trimmed == "/login" {
        app.show_interactive_login();
        return true;
    }

    if let Some(provider) = trimmed
        .strip_prefix("/login ")
        .or_else(|| trimmed.strip_prefix("/auth "))
    {
        let providers = crate::provider_catalog::tui_login_providers();
        if let Some(provider) =
            crate::provider_catalog::resolve_login_selection(provider, &providers)
        {
            app.start_login_provider(provider);
        } else {
            let valid = providers
                .iter()
                .map(|provider| provider.id)
                .collect::<Vec<_>>()
                .join(", ");
            app.push_display_message(DisplayMessage::error(format!(
                "Unknown provider '{}'. Use: {}",
                provider.trim(),
                valid
            )));
        }
        return true;
    }

    if trimmed == "/account" || trimmed == "/accounts" {
        app.show_accounts();
        return true;
    }

    if trimmed == "/account openai" || trimmed == "/accounts openai" {
        app.show_openai_accounts();
        return true;
    }

    if trimmed == "/subscription" || trimmed == "/subscription status" {
        app.show_jcode_subscription_status();
        return true;
    }

    if let Some(sub) = trimmed.strip_prefix("/account ") {
        if let Some(openai_sub) = sub.trim().strip_prefix("openai") {
            let openai_sub = openai_sub.trim();
            if openai_sub.is_empty() {
                app.show_openai_accounts();
                return true;
            }
            let parts: Vec<&str> = openai_sub.splitn(2, ' ').collect();
            match parts[0] {
                "list" | "ls" => app.show_openai_accounts(),
                "switch" | "use" => {
                    if let Some(label) = parts.get(1) {
                        app.switch_openai_account(label.trim());
                    } else {
                        app.push_display_message(DisplayMessage::error(
                            "Usage: `/account openai switch <label>`".to_string(),
                        ));
                    }
                }
                "add" | "login" => {
                    let label = parts.get(1).map(|s| s.trim()).unwrap_or("default");
                    app.start_openai_login_for_account(label);
                }
                "remove" | "rm" | "delete" => {
                    if let Some(label) = parts.get(1) {
                        app.remove_openai_account(label.trim());
                    } else {
                        app.push_display_message(DisplayMessage::error(
                            "Usage: `/account openai remove <label>`".to_string(),
                        ));
                    }
                }
                other => {
                    let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
                    if accounts.iter().any(|a| a.label == other) {
                        app.switch_openai_account(other);
                    } else {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Unknown OpenAI subcommand '{}'. Use: list, switch, add, remove",
                            other
                        )));
                    }
                }
            }
            return true;
        }

        let parts: Vec<&str> = sub.trim().splitn(2, ' ').collect();
        match parts[0] {
            "list" | "ls" => app.show_accounts(),
            "switch" | "use" => {
                if let Some(label) = parts.get(1) {
                    app.switch_account_by_label(label.trim());
                } else {
                    app.push_display_message(DisplayMessage::error(
                        "Usage: `/account switch <label>`".to_string(),
                    ));
                }
            }
            "add" | "login" => {
                let label = parts.get(1).map(|s| s.trim()).unwrap_or("default");
                app.start_claude_login_for_account(label);
            }
            "remove" | "rm" | "delete" => {
                if let Some(label) = parts.get(1) {
                    app.remove_account(label.trim());
                } else {
                    app.push_display_message(DisplayMessage::error(
                        "Usage: `/account remove <label>`".to_string(),
                    ));
                }
            }
            other => {
                let anthropic_accounts = crate::auth::claude::list_accounts().unwrap_or_default();
                let openai_accounts = crate::auth::codex::list_accounts().unwrap_or_default();
                if anthropic_accounts.iter().any(|a| a.label == other)
                    || openai_accounts.iter().any(|a| a.label == other)
                {
                    app.switch_account_by_label(other);
                } else {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Unknown subcommand '{}'. Use: list, switch, add, remove",
                        other
                    )));
                }
            }
        }
        return true;
    }

    false
}
