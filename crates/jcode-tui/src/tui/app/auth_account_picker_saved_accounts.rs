use super::*;

impl App {
    pub(crate) fn handle_login_picker_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> anyhow::Result<()> {
        use crate::tui::login_picker::OverlayAction;

        let action = {
            let Some(picker_cell) = self.login_picker_overlay.as_ref() else {
                return Ok(());
            };
            let mut picker = picker_cell.borrow_mut();
            picker.handle_overlay_key(code, modifiers)?
        };

        match action {
            OverlayAction::Continue => {}
            OverlayAction::Close => {
                self.login_picker_overlay = None;
            }
            OverlayAction::Execute(provider) => {
                self.login_picker_overlay = None;
                self.start_login_provider(provider);
            }
        }
        Ok(())
    }

    pub(crate) fn render_openai_accounts_markdown(&self) -> String {
        let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
        let active_label = crate::auth::codex::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();

        if accounts.is_empty() {
            return "OpenAI Accounts: none configured\n\n\
                 Use /account openai add to add the next numbered account, or /login openai to refresh the active one."
                .to_string();
        }

        let headers = ["Account", "Email", "Status", "ChatGPT Account ID", "Active"];
        let mut rows: Vec<[String; 5]> = Vec::new();
        for account in &accounts {
            let is_active = active_label.as_deref() == Some(&account.label);
            let status = match account.expires_at {
                Some(expires_at) if expires_at > now_ms => "valid",
                Some(_) => "expired",
                None => "valid",
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let account_id = account.account_id.as_deref().unwrap_or("unknown");
            let active_mark = if is_active { "active" } else { "" };
            rows.push([
                account.label.clone(),
                email,
                status.to_string(),
                account_id.to_string(),
                active_mark.to_string(),
            ]);
        }

        let mut lines = vec!["OpenAI Accounts:".to_string(), String::new()];
        lines.extend(format_account_table(&headers, &rows));
        lines.push(String::new());
        lines.push(
            "Commands: /account openai switch <label>, /account openai add, /account openai remove <label>"
                .to_string(),
        );

        lines.join("\n")
    }

    pub(crate) fn render_anthropic_accounts_markdown(&self) -> String {
        let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        let active_label = crate::auth::claude::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();

        if accounts.is_empty() {
            return "Anthropic Accounts: none configured\n\n\
                 Use /account claude add to add the next numbered account, or /login claude to refresh the active one."
                .to_string();
        }

        let headers = ["Account", "Email", "Status", "Subscription", "Active"];
        let mut rows: Vec<[String; 5]> = Vec::new();
        for account in &accounts {
            let is_active = active_label.as_deref() == Some(&account.label);
            let status = if account.expires > now_ms {
                "valid"
            } else {
                "expired"
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let sub = account.subscription_type.as_deref().unwrap_or("unknown");
            let active_mark = if is_active { "active" } else { "" };
            rows.push([
                account.label.clone(),
                email,
                status.to_string(),
                sub.to_string(),
                active_mark.to_string(),
            ]);
        }

        let mut lines = vec!["Anthropic Accounts:".to_string(), String::new()];
        lines.extend(format_account_table(&headers, &rows));
        lines.push(String::new());
        lines.push(
            "Commands: /account claude switch <label>, /account claude add, /account claude remove <label>"
                .to_string(),
        );

        lines.join("\n")
    }

    pub(super) fn append_anthropic_account_picker_items(
        &self,
        items: &mut Vec<crate::tui::account_picker::AccountPickerItem>,
        provider: crate::provider_catalog::LoginProviderDescriptor,
    ) {
        let active_label = crate::auth::claude::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();
        for account in crate::auth::claude::list_accounts().unwrap_or_default() {
            let status = if account.expires > now_ms {
                "valid"
            } else {
                "expired"
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let plan = account.subscription_type.as_deref().unwrap_or("unknown");
            let label = account.label.clone();
            let active_suffix = if active_label.as_deref() == Some(label.as_str()) {
                " - active"
            } else {
                ""
            };
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Switch account `{label}`"),
                format!("{email} - {status} - plan {plan}{active_suffix}"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} switch {}",
                    provider.id, label
                )),
            ));
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Re-login account `{label}`"),
                format!("Refresh OAuth tokens for `{label}`"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} add {}",
                    provider.id, label
                )),
            ));
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Remove account `{label}`"),
                format!("Delete saved credentials for `{label}`"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} remove {}",
                    provider.id, label
                )),
            ));
        }
    }

    pub(super) fn append_openai_account_picker_items(
        &self,
        items: &mut Vec<crate::tui::account_picker::AccountPickerItem>,
        provider: crate::provider_catalog::LoginProviderDescriptor,
    ) {
        let active_label = crate::auth::codex::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();
        for account in crate::auth::codex::list_accounts().unwrap_or_default() {
            let status = match account.expires_at {
                Some(expires_at) if expires_at > now_ms => "valid",
                Some(_) => "expired",
                None => "valid",
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let account_id = account.account_id.as_deref().unwrap_or("unknown");
            let label = account.label.clone();
            let active_suffix = if active_label.as_deref() == Some(label.as_str()) {
                " - active"
            } else {
                ""
            };
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Switch account `{label}`"),
                format!("{email} - {status} - acct {account_id}{active_suffix}"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} switch {}",
                    provider.id, label
                )),
            ));
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Re-login account `{label}`"),
                format!("Refresh OpenAI OAuth tokens for `{label}`"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} add {}",
                    provider.id, label
                )),
            ));
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Remove account `{label}`"),
                format!("Delete saved credentials for `{label}`"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} remove {}",
                    provider.id, label
                )),
            ));
        }
    }
}

fn format_account_table(headers: &[&str; 5], rows: &[[String; 5]]) -> Vec<String> {
    let mut widths = [0usize; 5];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = unicode_width::UnicodeWidthStr::width(*h);
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(unicode_width::UnicodeWidthStr::width(cell.as_str()));
        }
    }

    let render_row = |cells: &[String; 5]| -> String {
        let mut parts: Vec<String> = Vec::with_capacity(5);
        for (i, cell) in cells.iter().enumerate() {
            let pad =
                widths[i].saturating_sub(unicode_width::UnicodeWidthStr::width(cell.as_str()));
            parts.push(format!("{}{}", cell, " ".repeat(pad)));
        }
        format!("  {}", parts.join("  ").trim_end())
    };

    let header_cells: [String; 5] = std::array::from_fn(|i| headers[i].to_string());
    let mut lines = vec![render_row(&header_cells)];
    for row in rows {
        lines.push(render_row(row));
    }
    lines
}
