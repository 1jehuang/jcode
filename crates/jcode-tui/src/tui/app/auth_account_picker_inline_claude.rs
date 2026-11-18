use super::*;

impl App {
    pub(super) fn build_claude_inline_account_picker(
        &self,
    ) -> (Vec<crate::tui::PickerEntry>, usize) {
        let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        let active_label = self
            .window_account_label("claude")
            .unwrap_or_else(crate::auth::claude::primary_account_label);
        let next_label = crate::auth::claude::next_account_label()
            .unwrap_or_else(|_| crate::auth::claude::primary_account_label());
        let now_ms = chrono::Utc::now().timestamp_millis();

        let mut models = Vec::with_capacity(accounts.len() + 2);
        let mut selected = 0usize;

        for (index, account) in accounts.iter().enumerate() {
            let is_active = account.label == active_label;
            if is_active {
                selected = index;
            }
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
            models.push(crate::tui::PickerEntry {
                name: account_picker_display_name(
                    "Claude",
                    &account.label,
                    accounts.len(),
                    account.email.as_deref(),
                ),
                options: vec![crate::tui::PickerOption {
                    provider: "Claude".to_string(),
                    api_method: self
                        .account_row_badge("claude", &account.label)
                        .0
                        .to_string(),
                    available: true,
                    detail: format!(
                        "{} - {} - {} - plan {}",
                        email,
                        anthropic_account_use(account.subscription_type.as_deref()),
                        status,
                        plan
                    ),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::Account(
                    crate::tui::AccountPickerAction::Switch {
                        provider_id: "claude".to_string(),
                        label: account.label.clone(),
                    },
                ),
                selected_option: 0,
                is_current: is_active,
                is_default: false,
                is_favorite: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                usage_score: 0,
                old: false,
                created_date: None,
                effort: None,
            });
        }

        models.push(crate::tui::PickerEntry {
            name: "new account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Claude".to_string(),
                api_method: "new".to_string(),
                available: true,
                detail: format!("create {}", next_label),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Add {
                provider_id: "claude".to_string(),
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
        });

        let replace_target = accounts
            .iter()
            .find(|account| account.label == active_label)
            .map(|account| account.label.clone())
            .or_else(|| accounts.first().map(|account| account.label.clone()))
            .unwrap_or_else(crate::auth::claude::primary_account_label);
        models.push(crate::tui::PickerEntry {
            name: "replace account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Claude".to_string(),
                api_method: "replace".to_string(),
                available: !accounts.is_empty(),
                detail: if accounts.is_empty() {
                    "no saved accounts yet".to_string()
                } else {
                    format!("refresh {}", replace_target)
                },
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace {
                provider_id: "claude".to_string(),
                label: replace_target,
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
        });

        models.push(crate::tui::PickerEntry {
            name: "account center".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Claude".to_string(),
                api_method: "manage".to_string(),
                available: true,
                detail: "full Claude account center and settings".to_string(),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(
                crate::tui::AccountPickerAction::OpenCenter {
                    provider_filter: Some("claude".to_string()),
                },
            ),
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
        });

        if accounts.is_empty() {
            selected = 0;
        }
        (models, selected)
    }
}
