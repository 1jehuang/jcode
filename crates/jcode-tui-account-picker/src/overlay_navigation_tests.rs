use super::*;

fn key(picker: &mut AccountPicker, code: KeyCode) -> OverlayAction {
    picker
        .handle_overlay_key(code, KeyModifiers::empty())
        .unwrap()
}

fn filter(picker: &mut AccountPicker, text: &str) {
    for c in text.chars() {
        key(picker, KeyCode::Char(c));
    }
}

fn picker() -> AccountPicker {
    let mut items = vec![AccountPickerItem::action(
        "account-flow",
        "Accounts",
        "Add account",
        "Keep existing logins",
        AccountPickerCommand::OpenAddReplaceFlow {
            provider_filter: None,
        },
    )];
    for (provider_id, provider, label, remaining) in [
        ("claude", AccountProviderKind::Anthropic, "work one", 75),
        ("claude", AccountProviderKind::Anthropic, "work two", 50),
        ("claude", AccountProviderKind::Anthropic, "work three", 25),
        ("claude", AccountProviderKind::Anthropic, "work four", 0),
        ("openai", AccountProviderKind::OpenAi, "work one", 10),
    ] {
        let commands = [
            (
                "Switch",
                AccountPickerCommand::Switch {
                    provider: provider.clone(),
                    label: label.into(),
                },
            ),
            (
                "Reauthenticate",
                AccountPickerCommand::Login {
                    provider: provider.clone(),
                    label: label.into(),
                },
            ),
            (
                "Rename",
                AccountPickerCommand::Rename {
                    provider: provider.clone(),
                    label: label.into(),
                },
            ),
            (
                "Remove",
                AccountPickerCommand::Remove {
                    provider,
                    label: label.into(),
                },
            ),
        ];
        for (action, command) in commands {
            items.push(
                AccountPickerItem::action(
                    provider_id,
                    provider_id,
                    format!("{action} {label}"),
                    format!("{remaining}% left | {label}@example.invalid"),
                    command,
                )
                .with_details(vec![(
                    "Weekly remaining".into(),
                    format!("{remaining}% left"),
                )]),
            );
        }
    }
    items.push(AccountPickerItem::action(
        "openai",
        "OpenAI",
        "Transport",
        "auto",
        AccountPickerCommand::PromptValue {
            prompt: "Transport".into(),
            command_prefix: "/accounts openai transport".into(),
            empty_value: None,
            status_notice: "Editing transport".into(),
        },
    ));
    AccountPicker::new("Accounts", items).with_account_menus()
}

fn rendered(picker: &mut AccountPicker, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| picker.render(frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn accounts_root_has_one_entry_per_login_and_no_action_clumps() {
    let mut picker = picker();
    assert_eq!(picker.filtered.len(), 7);
    assert_eq!(
        picker
            .filtered
            .iter()
            .filter(|index| matches!(
                picker.items[**index].command,
                AccountPickerCommand::Switch {
                    provider: AccountProviderKind::Anthropic,
                    ..
                }
            ))
            .count(),
        4
    );
    assert!(picker.filtered.iter().all(|index| !matches!(
        picker.items[*index].command,
        AccountPickerCommand::Login { .. }
            | AccountPickerCommand::Rename { .. }
            | AccountPickerCommand::Remove { .. }
    )));
    for (width, height) in [(160, 45), (68, 22), (40, 12)] {
        let text = rendered(&mut picker, width, height);
        assert!(text.contains("Saved accounts"));
        assert!(!text.contains("Rename work"));
        assert!(!text.contains("Reauthenticate work"));
    }
}

#[test]
fn account_submenu_targets_exact_provider_and_restores_search_and_selection() {
    let mut picker = picker();
    filter(&mut picker, "claude work one");
    assert_eq!(picker.filtered.len(), 1);
    let selection = picker.selected;
    assert!(matches!(
        key(&mut picker, KeyCode::Enter),
        OverlayAction::Continue
    ));
    assert!(matches!(picker.view, AccountPickerView::Account(_)));
    assert_eq!(picker.filtered.len(), 5);
    for index in &picker.filtered {
        assert_eq!(
            account_target(&picker.items[*index].command),
            Some((&AccountProviderKind::Anthropic, "work one"))
        );
    }
    filter(&mut picker, "Rename");
    let OverlayAction::Execute(AccountPickerCommand::Rename { provider, label }) =
        key(&mut picker, KeyCode::Enter)
    else {
        panic!("expected typed rename")
    };
    assert_eq!(provider, AccountProviderKind::Anthropic);
    assert_eq!(label, "work one");
    key(&mut picker, KeyCode::Esc);
    assert_eq!(picker.filtered.len(), 5);
    key(&mut picker, KeyCode::Esc);
    assert_eq!(picker.view, AccountPickerView::Accounts);
    assert_eq!(picker.filter, "claude work one");
    assert_eq!(picker.selected, selection);
}

#[test]
fn live_usage_updates_selected_account_menu_without_leaking_other_accounts() {
    let mut picker = picker();
    filter(&mut picker, "claude work one");
    key(&mut picker, KeyCode::Enter);
    filter(&mut picker, "Rename");
    let selected = picker.selected;
    picker.update_account_details(|items| {
        for item in items {
            if matches!(&item.command, AccountPickerCommand::Switch { provider: AccountProviderKind::Anthropic, label } if label == "work one") {
                item.subtitle = "81% left | work one@example.invalid".into();
                item.details = vec![("Weekly remaining".into(), "81% left, reset tomorrow".into())];
            }
        }
    });
    assert_eq!(picker.selected, selected);
    assert_eq!(picker.filter, "Rename");
    let text = rendered(&mut picker, 160, 45);
    assert!(text.contains("81% left, reset tomorrow"));
    assert!(!text.contains("work two"));
    assert!(!text.contains("work three"));
    assert!(!text.contains("10% left"));
}

#[test]
fn account_menu_removal_still_needs_confirmation_and_escape_never_deletes() {
    let mut picker = picker();
    filter(&mut picker, "claude work two");
    key(&mut picker, KeyCode::Enter);
    filter(&mut picker, "Remove");
    assert!(matches!(
        key(&mut picker, KeyCode::Enter),
        OverlayAction::Continue
    ));
    assert!(picker.removal_confirmation.is_some());
    assert!(matches!(
        key(&mut picker, KeyCode::Enter),
        OverlayAction::Continue
    ));
    key(&mut picker, KeyCode::Esc);
    assert!(picker.removal_confirmation.is_none());
    assert!(matches!(picker.view, AccountPickerView::Account(_)));
    key(&mut picker, KeyCode::Enter);
    assert!(matches!(key(&mut picker, KeyCode::Char('y')),
        OverlayAction::Execute(AccountPickerCommand::Remove { provider: AccountProviderKind::Anthropic, label }) if label == "work two"));
}

#[test]
fn provider_controls_are_accessible_without_saved_account_action_clumps() {
    let mut picker = picker();
    filter(&mut picker, "Provider settings");
    assert_eq!(picker.filtered.len(), 1);
    key(&mut picker, KeyCode::Enter);
    assert_eq!(picker.view, AccountPickerView::ProviderSettings);
    assert_eq!(picker.filtered.len(), 1);
    assert_eq!(picker.selected_item().unwrap().title, "Transport");
    assert!(matches!(
        key(&mut picker, KeyCode::Enter),
        OverlayAction::Execute(AccountPickerCommand::PromptValue { .. })
    ));
    key(&mut picker, KeyCode::Esc);
    assert_eq!(picker.view, AccountPickerView::Accounts);
    assert_eq!(picker.filter, "Provider settings");
}
