use super::*;

/// The login picker offers explicit additions alongside reauthentication of saved
/// credentials. Both use the same account actions as the `/account` picker.
pub(super) fn saved_login_picker_entries(
    current_provider: &str,
    selected_label: Option<&str>,
) -> Vec<crate::tui::PickerEntry> {
    let mut entries = Vec::new();
    let now = chrono::Utc::now().timestamp_millis();
    let active = selected_login_label(
        "claude",
        current_provider,
        selected_label,
        crate::auth::claude::active_account_label(),
    );
    let accounts = match crate::auth::claude::list_accounts() {
        Ok(accounts) => accounts,
        Err(error) => {
            crate::logging::warn(&format!(
                "Could not load saved Claude login choices: {error}"
            ));
            Vec::new()
        }
    };
    for account in accounts {
        entries.push(saved_login_entry(
            "claude",
            "Claude",
            &account.label,
            account.email.as_deref(),
            anthropic_account_use(account.subscription_type.as_deref()),
            account.expires <= now,
            active.as_deref() == Some(account.label.as_str()),
        ));
    }
    let active = selected_login_label(
        "openai",
        current_provider,
        selected_label,
        crate::auth::codex::active_account_label(),
    );
    let accounts = match crate::auth::codex::list_accounts() {
        Ok(accounts) => accounts,
        Err(error) => {
            crate::logging::warn(&format!(
                "Could not load saved OpenAI login choices: {error}"
            ));
            Vec::new()
        }
    };
    for account in accounts {
        entries.push(saved_login_entry(
            "openai",
            "OpenAI",
            &account.label,
            account.email.as_deref(),
            "saved",
            account.expires_at.is_some_and(|expires| expires <= now),
            active.as_deref() == Some(account.label.as_str()),
        ));
    }
    entries.extend(new_login_account_entries());
    entries
}

/// Allocate no labels while rendering. The existing account-add action allocates
/// an unused label at selection time and aborts on allocation errors.
fn new_login_account_entries() -> Vec<crate::tui::PickerEntry> {
    use crate::tui::{AccountPickerAction, PickerAction, PickerEntry, PickerOption};
    [("claude", "Claude"), ("openai", "OpenAI")]
        .into_iter()
        .map(|(provider_id, provider_name)| PickerEntry {
            name: format!("{provider_name} · Add another account"),
            options: vec![PickerOption {
                provider: provider_id.to_string(),
                api_method: "new account".to_string(),
                available: true,
                detail: "Sign in with a new account label without replacing saved credentials"
                    .to_string(),
                estimated_reference_cost_micros: None,
            }],
            action: PickerAction::Account(AccountPickerAction::Add {
                provider_id: provider_id.to_string(),
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        })
        .collect()
}

fn selected_login_label(
    provider: &str,
    current_provider: &str,
    bound: Option<&str>,
    saved: Option<String>,
) -> Option<String> {
    let matches = current_provider.eq_ignore_ascii_case(provider)
        || (provider == "claude" && current_provider.eq_ignore_ascii_case("anthropic"));
    if matches {
        bound.map(str::to_owned).or(saved)
    } else {
        saved
    }
}

#[cfg(test)]
mod saved_account_current_login_tests {
    use super::*;
    #[test]
    fn session_account_takes_precedence_over_global_saved_preference() {
        assert_eq!(
            selected_login_label("openai", "OpenAI", Some("work"), Some("personal".into()))
                .as_deref(),
            Some("work")
        );
        assert_eq!(
            selected_login_label("claude", "Anthropic", Some("personal"), Some("work".into()))
                .as_deref(),
            Some("personal")
        );
        assert_eq!(
            selected_login_label("openai", "Claude", Some("work"), Some("personal".into()))
                .as_deref(),
            Some("personal")
        );
    }
}

fn saved_login_entry(
    provider_id: &str,
    provider_name: &str,
    label: &str,
    email: Option<&str>,
    account_use: &str,
    expired: bool,
    active: bool,
) -> crate::tui::PickerEntry {
    use crate::tui::{AccountPickerAction, PickerAction, PickerEntry, PickerOption};
    PickerEntry {
        name: format!(
            "{provider_name} · {label} · {}",
            email.unwrap_or("email unknown")
        ),
        options: vec![PickerOption {
            provider: provider_id.to_string(),
            api_method: "sign in again".to_string(),
            available: true,
            detail: format!(
                "Sign in again to {label} · {account_use} · {}{}",
                if expired { "expired" } else { "valid" },
                if active { " · active saved login" } else { "" }
            ),
            estimated_reference_cost_micros: None,
        }],
        action: PickerAction::Account(AccountPickerAction::Replace {
            provider_id: provider_id.to_string(),
            label: label.to_string(),
        }),
        selected_option: 0,
        is_current: active,
        is_default: false,
        is_favorite: false,
        recommended: false,
        recommendation_rank: usize::MAX,
        usage_score: 0,
        old: false,
        created_date: None,
        effort: None,
    }
}

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
        lines.push("## ChatGPT OAuth API-equivalent usage".to_string());
        lines.push("Today is since local midnight. Lifetime is recorded Jcode usage, not your subscription bill or all ChatGPT activity.".to_string());
        for account in &accounts {
            lines.push(String::new());
            lines.push(format!(
                "### {} (`{}`)",
                account_display_name("OpenAI", &account.label, accounts.len()),
                account.label
            ));
            for (label, value) in
                crate::provider_activity::openai_oauth_usage_summary(&account.label)
            {
                lines.push(format!("- **{label}:** {value}"));
            }
        }

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
            items.push(
                crate::tui::account_picker::AccountPickerItem::action(
                    provider.id,
                    provider.display_name,
                    format!("Switch {display_name}"),
                    format!("{email} - {status} - acct {account_id}{active_suffix}"),
                    crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                        "/account {} switch {}",
                        provider.id, label
                    )),
                )
                .with_details(openai_account_usage_details(&label)),
            );
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

/// Keep full usage out of compact list subtitles, which are intentionally truncated.
fn openai_account_usage_details(label: &str) -> Vec<(String, String)> {
    let mut details = vec![(
        "Full usage details".to_string(),
        "/account openai settings".to_string(),
    )];
    details.extend(crate::provider_activity::openai_oauth_usage_summary(label));
    details
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

/// Show the full login identity in the local picker so similarly named accounts
/// can be distinguished without opening provider settings.
pub(super) fn account_picker_display_name(
    provider: &str,
    label: &str,
    account_count: usize,
    email: Option<&str>,
) -> String {
    let name = account_display_name(provider, label, account_count);
    match email.map(str::trim).filter(|email| !email.is_empty()) {
        Some(email) => format!("{name} · {email}"),
        None => format!("{name} · email unknown"),
    }
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

#[cfg(test)]
mod account_display_tests {
    use super::*;

    #[test]
    fn login_additions_use_safe_account_add_actions_for_both_oauth_providers() {
        let entries = new_login_account_entries();
        assert_eq!(entries.len(), 2);
        for (entry, provider) in entries.iter().zip(["claude", "openai"]) {
            assert!(entry.name.contains("Add another account"));
            assert_eq!(entry.options.len(), 1);
            assert!(entry.options[0].available);
            assert_eq!(entry.options[0].api_method, "new account");
            assert!(!entry.is_current);
            assert!(!entry.is_default);
            assert!(matches!(&entry.action,
                crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Add { provider_id })
                if provider_id == provider));
        }
    }

    #[test]
    fn login_picker_includes_additions_independently_of_saved_account_state() {
        let entries = saved_login_picker_entries("OpenAI", Some("existing-account"));
        for provider in ["claude", "openai"] {
            assert_eq!(entries.iter().filter(|entry| matches!(&entry.action,
                crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Add { provider_id })
                if provider_id == provider)).count(), 1);
        }
    }

    #[test]
    fn saved_login_choices_preserve_all_named_reauthentication_targets() {
        for provider in ["claude", "openai"] {
            for label in ["work", "personal", "custom-named-account"] {
                let entry = saved_login_entry(
                    provider,
                    provider,
                    label,
                    Some("me@example.com"),
                    "work",
                    false,
                    true,
                );
                assert!(entry.name.contains(label));
                assert!(entry.name.contains("me@example.com"));
                assert!(entry.is_current);
                assert_eq!(entry.options[0].api_method, "sign in again");
                assert!(entry.options[0].detail.contains("Sign in again"));
                assert!(matches!(entry.action,
                    crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace { provider_id, label: target })
                    if provider_id == provider && target == label));
            }
        }
    }

    #[test]
    fn saved_login_lists_distinguish_active_and_inactive_accounts_for_both_providers() {
        for provider in ["claude", "openai"] {
            let entries = [
                saved_login_entry(
                    provider,
                    provider,
                    "personal",
                    Some("personal@example.com"),
                    "personal",
                    false,
                    true,
                ),
                saved_login_entry(
                    provider,
                    provider,
                    "work",
                    Some("work@example.com"),
                    "work",
                    false,
                    false,
                ),
            ];
            assert_eq!(entries.iter().filter(|entry| entry.is_current).count(), 1);
            assert!(entries[0].options[0].detail.contains("active saved login"));
            assert!(entries[1].name.contains("work@example.com"));
            assert!(matches!(&entries[1].action,
                crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace { provider_id, label })
                if provider_id == provider && label == "work"));
        }
    }

    #[test]
    fn expired_saved_login_starts_reauthentication_for_the_same_account() {
        let entry = saved_login_entry("claude", "Claude", "work", None, "work", true, false);
        assert!(entry.options[0].available);
        assert!(entry.options[0].detail.contains("expired"));
        assert!(!entry.is_current);
        assert!(entry.name.contains("email unknown"));
        assert!(matches!(entry.action,
            crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace { provider_id, label })
            if provider_id == "claude" && label == "work"));
    }

    #[test]
    fn picker_names_show_full_email_for_single_and_multiple_accounts() {
        assert_eq!(
            account_picker_display_name("Claude", "claude-otter", 1, Some("personal@example.com")),
            "Claude · personal@example.com"
        );
        assert_eq!(
            account_picker_display_name("OpenAI", "openai-fox", 2, Some("work@example.com")),
            "OpenAI Fox · work@example.com"
        );
    }

    #[test]
    fn picker_names_handle_missing_and_blank_emails() {
        for email in [None, Some(""), Some("  ")] {
            assert_eq!(
                account_picker_display_name("Claude", "claude-otter", 2, email),
                "Claude Otter · email unknown"
            );
        }
    }

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
