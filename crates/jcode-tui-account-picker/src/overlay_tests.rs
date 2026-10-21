use super::*;
use ratatui::{Terminal, backend::TestBackend, widgets::Paragraph};

fn regression_items() -> Vec<AccountPickerItem> {
    (0..12)
        .map(|index| {
            AccountPickerItem::action(
                format!("provider-{index}"),
                format!("Provider {index}"),
                format!("Action {index}"),
                "long subtitle that must never wrap into a mouse row",
                AccountPickerCommand::SubmitInput(format!("/accounts p{index} settings")),
            )
        })
        .collect()
}

#[test]
fn account_focus_and_usage_updates_preserve_search_and_target() {
    let items = ["personal", "work account"]
        .into_iter()
        .map(|label| {
            AccountPickerItem::action(
                "openai",
                "OpenAI",
                format!("Use {label} in this window"),
                "Usage loading",
                AccountPickerCommand::Switch {
                    provider: AccountProviderKind::OpenAi,
                    label: label.into(),
                },
            )
        })
        .collect();
    let mut picker = AccountPicker::new("Accounts", items);
    picker.select_account("openai", Some("work account"));
    assert!(matches!(&picker.selected_item().unwrap().command,
        AccountPickerCommand::Switch { label, .. } if label == "work account"));
    for letter in "work".chars() {
        picker
            .handle_overlay_key(KeyCode::Char(letter), KeyModifiers::empty())
            .unwrap();
    }
    let before = (
        picker.selected,
        picker.filtered.clone(),
        picker.filter.clone(),
    );
    picker.update_account_details(|items| {
        for item in items {
            item.subtitle = "5h 75% left | saved".into();
            item.details = vec![("5h".into(), "75% left; resets tomorrow".into())];
        }
    });
    assert_eq!(
        (
            picker.selected,
            picker.filtered.clone(),
            picker.filter.clone()
        ),
        before
    );
    assert!(
        matches!(picker.handle_overlay_key(KeyCode::Enter, KeyModifiers::empty()).unwrap(),
        OverlayAction::Execute(AccountPickerCommand::Switch { label, .. }) if label == "work account")
    );
}

#[test]
fn usage_updates_preserve_exact_pending_removal_confirmation() {
    let mut picker = AccountPicker::new(
        "Accounts",
        vec![AccountPickerItem::action(
            "openai",
            "OpenAI",
            "Remove work",
            "saved",
            AccountPickerCommand::Remove {
                provider: AccountProviderKind::OpenAi,
                label: "work".into(),
            },
        )],
    );
    picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    picker.update_account_details(|items| items[0].subtitle = "updated metadata".into());
    assert!(
        matches!(picker.handle_overlay_key(KeyCode::Char('y'), KeyModifiers::empty()).unwrap(),
        OverlayAction::Execute(AccountPickerCommand::Remove { label, .. }) if label == "work")
    );
}

#[test]
fn remaining_quota_survives_long_account_names_and_scope_badges() {
    let item = AccountPickerItem::action(
        "openai",
        "OpenAI",
        "Use a very long custom account name in this window",
        "5h 75% left · 7d 25% left | test@example.com - this window - pinned - default",
        AccountPickerCommand::Switch {
            provider: AccountProviderKind::OpenAi,
            label: "a very long custom account name".into(),
        },
    );
    for width in [38, 50, 64, 90] {
        let mut picker = AccountPicker::new("Accounts", vec![item.clone()]);
        let mut terminal = Terminal::new(TestBackend::new(width, 8)).unwrap();
        terminal
            .draw(|frame| picker.render_action_list(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = (0..8)
            .flat_map(|y| (0..width).map(move |x| buffer[(x, y)].symbol()))
            .collect::<String>();
        assert!(
            text.contains("5h 75% left"),
            "quota hidden at width {width}: {text}"
        );
    }
}

#[test]
fn physical_rows_keep_every_selection_visible_and_map_mouse_exactly() {
    let mut picker = AccountPicker::new("Accounts", regression_items());
    for height in 1..9 {
        for selection in 0..picker.filtered.len() {
            picker.selected = selection;
            let rows = picker.visible_rows(height);
            assert!(rows.len() <= height as usize);
            assert!(rows.contains(&(selection, false)));
            let mut terminal = Terminal::new(TestBackend::new(32, height + 2)).unwrap();
            terminal
                .draw(|frame| picker.render_action_list(frame, frame.area()))
                .unwrap();
            let area = picker.last_action_list_area.unwrap();
            for (row, &(index, header)) in rows.iter().enumerate() {
                assert_eq!(
                    picker.visible_index_for_action_row(row as u16, height),
                    (!header).then_some(index)
                );
                let marker = terminal.backend().buffer()[(area.x, area.y + row as u16)].symbol();
                assert_eq!(
                    marker,
                    if index == selection && !header {
                        ">"
                    } else {
                        " "
                    }
                );
                picker.selected = selection;
                picker.handle_overlay_mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: area.x,
                    row: area.y + row as u16,
                    modifiers: KeyModifiers::empty(),
                });
                assert_eq!(picker.selected, if header { selection } else { index });
                picker.selected = selection;
            }
        }
    }
}

#[test]
fn search_letters_and_empty_enter_never_execute() {
    let mut picker = AccountPicker::new("Accounts", regression_items());
    for letter in "Google work jqk default".chars() {
        assert!(matches!(
            picker
                .handle_overlay_key(KeyCode::Char(letter), KeyModifiers::empty())
                .unwrap(),
            OverlayAction::Continue
        ));
    }
    assert_eq!(picker.filter, "Google work jqk default");
    assert!(picker.filtered.is_empty());
    assert!(matches!(
        picker
            .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap(),
        OverlayAction::Continue
    ));
    for code in [
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
    ] {
        picker
            .handle_overlay_key(code, KeyModifiers::empty())
            .unwrap();
        assert_eq!(picker.selected, 0);
    }
    picker
        .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
        .unwrap();
    for code in [KeyCode::End, KeyCode::PageDown] {
        picker
            .handle_overlay_key(code, KeyModifiers::empty())
            .unwrap();
        assert_eq!(picker.selected, 11);
    }
    picker
        .handle_overlay_key(KeyCode::Home, KeyModifiers::empty())
        .unwrap();
    picker
        .handle_overlay_key(KeyCode::PageUp, KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.selected, 0);
}

#[test]
fn removal_requires_y_and_changes_disarm_for_both_command_forms() {
    for command in [
        AccountPickerCommand::Remove {
            provider: AccountProviderKind::OpenAi,
            label: "exact-work".into(),
        },
        AccountPickerCommand::SubmitInput("/accounts openai remove exact-work".into()),
    ] {
        let item =
            AccountPickerItem::action("openai", "OpenAI", "Remove exact-work", "saved", command);
        let mut picker = AccountPicker::new("Accounts", vec![item]);
        for cancel in [
            KeyCode::Esc,
            KeyCode::Down,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Backspace,
        ] {
            assert!(matches!(
                picker
                    .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                    .unwrap(),
                OverlayAction::Continue
            ));
            assert!(matches!(
                picker
                    .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                    .unwrap(),
                OverlayAction::Continue
            ));
            assert!(picker.removal_confirmation.is_some());
            picker
                .handle_overlay_key(cancel, KeyModifiers::empty())
                .unwrap();
            assert!(picker.removal_confirmation.is_none());
        }
        picker
            .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| picker.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let text = (0..24)
            .flat_map(|y| (0..80).map(move |x| buffer[(x, y)].symbol()))
            .collect::<String>();
        assert!(text.contains("exact-work"));
        assert!(text.contains("y: confirm removal"));
        picker.handle_overlay_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::empty(),
        });
        assert!(picker.removal_confirmation.is_none());
        picker
            .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
        assert!(matches!(
            picker
                .handle_overlay_key(KeyCode::Char('y'), KeyModifiers::empty())
                .unwrap(),
            OverlayAction::Execute(_)
        ));
        let mut picker = picker.clone();
        picker
            .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
        picker
            .handle_overlay_key(KeyCode::Char('w'), KeyModifiers::empty())
            .unwrap();
        assert!(picker.removal_confirmation.is_none());
    }
}

#[test]
fn compact_titles_preserve_login_intent_and_window_scope_in_details() {
    for title in ["Sign in", "Refresh login"] {
        let item = AccountPickerItem::action(
            "openai",
            "OpenAI",
            title,
            "OAuth",
            AccountPickerCommand::SubmitInput("/accounts openai login".into()),
        );
        assert_eq!(compact_item_title(&item), title);
    }
    let item = AccountPickerItem::action(
        "openai",
        "OpenAI",
        "Use work in this window",
        "saved",
        AccountPickerCommand::Switch {
            provider: AccountProviderKind::OpenAi,
            label: "work".into(),
        },
    );
    assert_eq!(compact_item_title(&item), "work");
    assert!(item.title.contains("in this window"));
    let mut picker = AccountPicker::new("Accounts", vec![item]);
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    terminal.draw(|frame| picker.render(frame)).unwrap();
    assert!(picker.last_action_list_area.unwrap().height >= 1);
}

#[test]
fn saved_account_enter_uses_in_window_and_ctrl_d_sets_default() {
    let item = AccountPickerItem::action(
        "claude",
        "Claude",
        "Switch account `claude-fox`",
        "saved",
        AccountPickerCommand::Switch {
            provider: AccountProviderKind::Anthropic,
            label: "claude-fox".into(),
        },
    );
    let mut picker = AccountPicker::new("Accounts", vec![item.clone()]);
    assert!(matches!(
        picker.handle_overlay_key(KeyCode::Char('d'), KeyModifiers::CONTROL).unwrap(),
        OverlayAction::Execute(AccountPickerCommand::SetDefault {
            provider: AccountProviderKind::Anthropic,
            ref label,
        }) if label == "claude-fox"
    ));
    let mut picker = AccountPicker::new("Accounts", vec![item]);
    assert!(matches!(
        picker.handle_overlay_key(KeyCode::Enter, KeyModifiers::empty()).unwrap(),
        OverlayAction::Execute(AccountPickerCommand::Switch { ref label, .. })
            if label == "claude-fox"
    ));
    // With a filter typed, `d` keeps editing the filter.
    let mut picker = AccountPicker::new(
        "Accounts",
        vec![AccountPickerItem::action(
            "claude",
            "Claude",
            "Switch account `claude-fox`",
            "saved default",
            AccountPickerCommand::Switch {
                provider: AccountProviderKind::Anthropic,
                label: "claude-fox".into(),
            },
        )],
    );
    picker
        .handle_overlay_key(KeyCode::Char('f'), KeyModifiers::empty())
        .unwrap();
    assert!(matches!(
        picker
            .handle_overlay_key(KeyCode::Char('d'), KeyModifiers::empty())
            .unwrap(),
        OverlayAction::Continue
    ));
}

#[test]
fn saved_account_badges_mark_window_default_and_pinned() {
    let item = |subtitle: &str| {
        AccountPickerItem::action(
            "claude",
            "Claude",
            "Switch account `claude-fox`",
            subtitle,
            AccountPickerCommand::Switch {
                provider: AccountProviderKind::Anthropic,
                label: "claude-fox".into(),
            },
        )
    };
    assert_eq!(
        render_support::account_badges(&item("a@b - this window - pinned - default")),
        vec!["this window", "pinned", "default"]
    );
    assert!(render_support::account_badges(&item("a@b - valid")).is_empty());
    assert!(account_is_active(&item("a@b - this window")));
}

#[test]
fn account_usage_details_wrap_without_changing_switch_action() {
    let mut picker = AccountPicker::new(
        "Accounts",
        vec![
            AccountPickerItem::action(
                "openai",
                "OpenAI",
                "Switch OpenAI Otter",
                "saved",
                AccountPickerCommand::SubmitInput("/accounts openai switch openai-otter".into()),
            )
            .with_details(vec![
                (
                    "Full usage details".into(),
                    "/accounts openai settings".into(),
                ),
                (
                    "Today (local midnight)".into(),
                    "123456789 input / 450000 output tokens (60000 cached input), $1.2500 known + unknown cost (2 unpriced responses) API-equivalent estimate, not a bill; recorded only; partial token counts; since local midnight"
                        .into(),
                ),
                (
                    "Lifetime".into(),
                    "1234567890 input / 4500000 output tokens (600000 cached input), $12.5000 API-equivalent estimate, not a bill; recorded only; since 2026-09-06 12:34 -07:00"
                        .into(),
                ),
                (
                    "Estimate".into(),
                    "Not your subscription bill. Recorded Jcode usage only.".into(),
                ),
            ]),
        ],
    );
    let mut terminal = Terminal::new(TestBackend::new(42, 38)).unwrap();
    terminal
        .draw(|frame| picker.render_detail_pane(frame, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rendered = (1..37)
        .map(|y| {
            (1..41)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(" ");
    for expected in [
        "Today",
        "Lifetime",
        "$1.25",
        "$12.50",
        "1234567890 input",
        "600000 cached input",
        "Not your subscription bill.",
        "/accounts openai settings",
    ] {
        assert!(
            rendered.contains(expected),
            "missing {expected}: {rendered}"
        );
    }
    assert!(!rendered.contains('…'));
    let mut compact = Terminal::new(TestBackend::new(42, 10)).unwrap();
    compact
        .draw(|frame| picker.render_detail_pane(frame, frame.area()))
        .unwrap();
    let compact_buffer = compact.backend().buffer();
    let compact_text = (0..10)
        .flat_map(|y| (0..42).map(move |x| compact_buffer[(x, y)].symbol()))
        .collect::<String>();
    assert!(
        compact_text.contains("/accounts openai settings"),
        "short panes retain the full-detail route"
    );
    assert!(
        matches!(picker.handle_overlay_key(KeyCode::Enter, KeyModifiers::empty()).unwrap(),
        OverlayAction::Execute(AccountPickerCommand::SubmitInput(command)) if command == "/accounts openai switch openai-otter")
    );
}

#[test]
fn test_account_picker_preserves_underlying_background_outside_panels() {
    let mut picker = AccountPicker::new(
        " Accounts ",
        vec![AccountPickerItem::action(
            "openai",
            "OpenAI",
            "Add account",
            "Start login flow",
            AccountPickerCommand::SubmitInput("/accounts openai add default".to_string()),
        )],
    );

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            let fill = vec![Line::from("X".repeat(area.width as usize)); area.height as usize];
            frame.render_widget(Paragraph::new(fill), area);
            picker.render(frame);
        })
        .expect("draw failed");

    let overlay = centered_rect(
        OVERLAY_PERCENT_X,
        OVERLAY_PERCENT_Y,
        Rect::new(0, 0, 100, 30),
    );
    let probe = &terminal.backend().buffer()[(overlay.x.saturating_sub(1), overlay.y)];
    assert_eq!(probe.symbol(), "X");
    assert_ne!(probe.bg, Color::Rgb(18, 21, 30));
}

#[test]
fn test_account_picker_mouse_click_selects_visible_action_after_group_header() {
    let mut picker = AccountPicker::new(
        " Accounts ",
        vec![
            AccountPickerItem::action(
                "openai",
                "OpenAI",
                "Provider settings",
                "configured",
                AccountPickerCommand::SubmitInput("/accounts openai settings".to_string()),
            ),
            AccountPickerItem::action(
                "openai",
                "OpenAI",
                "Login / refresh",
                "OAuth",
                AccountPickerCommand::SubmitInput("/accounts openai login".to_string()),
            ),
        ],
    );

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("draw failed");

    let list_area = picker
        .last_action_list_area
        .expect("render should record action list area");

    let initially_selected = picker.selected;
    picker.handle_overlay_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: list_area.x + 1,
        row: list_area.y,
        modifiers: KeyModifiers::empty(),
    });
    assert_eq!(
        picker.selected, initially_selected,
        "provider group header rows should not be selectable"
    );

    let expected_first_action = picker.items[picker.filtered[0]].title.clone();
    // Row 0 is the provider group header; row 1 is the first sorted action.
    picker.handle_overlay_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: list_area.x + 1,
        row: list_area.y + 1,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(
        picker.selected_item().map(|item| item.title.as_str()),
        Some(expected_first_action.as_str())
    );
}

#[test]
fn test_prompt_value_command_preview_shows_placeholder() {
    let preview = command_preview(&AccountPickerCommand::PromptValue {
        prompt: "Enter default model".to_string(),
        command_prefix: "/accounts default-model".to_string(),
        empty_value: Some("clear".to_string()),
        status_notice: "editing".to_string(),
    });

    assert!(preview.contains("/accounts default-model <value>"));
    assert!(preview.contains("clear"));
}

#[test]
fn test_account_picker_sorts_switches_before_settings() {
    let picker = AccountPicker::new(
        " Accounts ",
        vec![
            AccountPickerItem::action(
                "openai",
                "OpenAI",
                "Provider settings",
                "configured",
                AccountPickerCommand::SubmitInput("/accounts openai settings".to_string()),
            ),
            AccountPickerItem::action(
                "openai",
                "OpenAI",
                "Switch account `work`",
                "user@example.com - valid - active",
                AccountPickerCommand::SubmitInput("/accounts openai switch work".to_string()),
            ),
            AccountPickerItem::action(
                "defaults",
                "Global",
                "Default provider",
                "Current: auto",
                AccountPickerCommand::PromptValue {
                    prompt: "provider".to_string(),
                    command_prefix: "/accounts default-provider".to_string(),
                    empty_value: Some("auto".to_string()),
                    status_notice: "editing".to_string(),
                },
            ),
        ],
    );

    let ordered_titles: Vec<String> = picker
        .filtered
        .iter()
        .map(|idx| picker.items[*idx].title.clone())
        .collect();

    assert_eq!(ordered_titles[0], "Switch account `work`");
    assert_eq!(ordered_titles[1], "Provider settings");
    assert_eq!(ordered_titles[2], "Default provider");
}

#[test]
fn test_account_picker_left_right_jump_by_provider_group() {
    let mut picker = AccountPicker::new(
        " Accounts ",
        vec![
            AccountPickerItem::action(
                "claude",
                "Claude",
                "Switch account `work`",
                "a@example.com - valid - active",
                AccountPickerCommand::SubmitInput("/accounts claude switch work".to_string()),
            ),
            AccountPickerItem::action(
                "claude",
                "Claude",
                "Provider settings",
                "configured",
                AccountPickerCommand::SubmitInput("/accounts claude settings".to_string()),
            ),
            AccountPickerItem::action(
                "openai",
                "OpenAI",
                "Switch account `default`",
                "b@example.com - valid - active",
                AccountPickerCommand::SubmitInput("/accounts openai switch default".to_string()),
            ),
        ],
    );

    picker.selected = 1;
    let _ = picker.handle_overlay_key(KeyCode::Right, KeyModifiers::empty());
    assert_eq!(
        picker.items[picker.filtered[picker.selected]].provider_id,
        "openai"
    );

    let _ = picker.handle_overlay_key(KeyCode::Left, KeyModifiers::empty());
    assert_eq!(
        picker.items[picker.filtered[picker.selected]].provider_id,
        "claude"
    );
    assert_eq!(picker.selected, 0);
}
