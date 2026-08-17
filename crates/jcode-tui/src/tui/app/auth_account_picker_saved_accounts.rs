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
                 Use /account openai add to add another account, or /login openai to refresh the active one."
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
                account_display_name("OpenAI", &account.label, accounts.len()),
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
                 Use /account claude add to add another account, or /login claude to refresh the active one."
                .to_string();
        }

        let headers = ["Account", "Email", "Status", "Use", "Subscription"];
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
            let account_use = anthropic_account_use(account.subscription_type.as_deref());
            let sub = if is_active {
                format!("{sub} (active)")
            } else {
                sub.to_string()
            };
            rows.push([
                account_display_name("Claude", &account.label, accounts.len()),
                email,
                status.to_string(),
                account_use.to_string(),
                sub,
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
        let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        for account in &accounts {
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
            let account_use = anthropic_account_use(account.subscription_type.as_deref());
            let label = account.label.clone();
            let display_name = account_display_name("Claude", &label, accounts.len());
            let active_suffix = if active_label.as_deref() == Some(label.as_str()) {
                " - active"
            } else {
                ""
            };
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Switch {display_name}"),
                format!("{email} - {account_use} - {status} - plan {plan}{active_suffix}"),
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
        let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
        for account in &accounts {
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
            let display_name = account_display_name("OpenAI", &label, accounts.len());
            let active_suffix = if active_label.as_deref() == Some(label.as_str()) {
                " - active"
            } else {
                ""
            };
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Switch {display_name}"),
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

/// A provider name is enough when there is only one login. Animal names are
/// useful only when multiple logins of that provider need distinguishing.
pub(super) fn account_display_name(provider: &str, label: &str, account_count: usize) -> String {
    if account_count <= 1 {
        return provider.to_string();
    }
    let animal = label
        .rsplit_once('-')
        .map(|(_, animal)| animal)
        .unwrap_or(label);
    let mut chars = animal.chars();
    let animal = chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_else(|| "Account".to_string());
    format!("{provider} {animal}")
}

/// Anthropic exposes the subscription kind but not an explicit work/personal
/// flag. Team and enterprise plans are organizational; individual plans are
/// personal. Unknown values stay unknown rather than being guessed from email.
pub(super) fn anthropic_account_use(subscription_type: Option<&str>) -> &'static str {
    match subscription_type.map(str::to_ascii_lowercase).as_deref() {
        Some("team" | "enterprise" | "business") => "work",
        Some("free" | "pro" | "max") => "personal",
        _ => "unknown",
    }
}

#[cfg(test)]
mod account_display_tests {
    use super::*;

    #[test]
    fn animals_only_distinguish_duplicate_provider_logins() {
        assert_eq!(account_display_name("Claude", "claude-otter", 1), "Claude");
        assert_eq!(
            account_display_name("Claude", "claude-otter", 2),
            "Claude Otter"
        );
        assert_eq!(
            account_display_name("Claude", "claude-fox", 2),
            "Claude Fox"
        );
    }

    #[test]
    fn known_anthropic_plans_identify_personal_and_work_accounts() {
        assert_eq!(anthropic_account_use(Some("max")), "personal");
        assert_eq!(anthropic_account_use(Some("team")), "work");
        assert_eq!(anthropic_account_use(None), "unknown");
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
