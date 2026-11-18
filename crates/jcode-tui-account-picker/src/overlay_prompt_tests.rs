use super::*;

fn picker() -> AccountPicker {
    let mut items = Vec::new();
    for (id, provider) in [
        ("claude", AccountProviderKind::Anthropic),
        ("openai", AccountProviderKind::OpenAi),
    ] {
        for label in ["shared", "other"] {
            items.push(AccountPickerItem::action(
                id,
                id,
                format!("Use {label} in this window"),
                "saved login",
                AccountPickerCommand::Switch {
                    provider: provider.clone(),
                    label: label.into(),
                },
            ));
        }
    }
    AccountPicker::new("Accounts", items).with_account_menus()
}

fn key(picker: &mut AccountPicker, code: KeyCode) -> OverlayAction {
    picker
        .handle_overlay_key(code, KeyModifiers::empty())
        .unwrap()
}

fn render(picker: &mut AccountPicker, width: u16, height: u16) -> String {
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
fn manager_scope_distinguishes_flat_global_and_scoped() {
    assert!(!AccountPicker::new("Add", vec![]).is_manager_subpage());
    assert!(!picker().is_manager_subpage());
    assert!(
        AccountPicker::new("Add", vec![])
            .with_manager_scope(None)
            .is_manager_subpage()
    );
    assert_eq!(AccountPicker::new("Add", vec![]).manager_scope(), None);
    assert_eq!(picker().manager_scope(), Some(None));
    assert_eq!(
        picker()
            .with_manager_scope(Some("openai".into()))
            .manager_scope(),
        Some(Some("openai".into()))
    );
    assert_eq!(
        AccountPicker::new("Add", vec![])
            .with_manager_scope(None)
            .manager_scope(),
        Some(None)
    );
    assert_eq!(
        AccountPicker::new("Accounts", vec![])
            .with_manager_scope(Some("claude".into()))
            .with_account_menus()
            .manager_scope(),
        Some(Some("claude".into()))
    );
}

#[test]
fn unicode_editing_and_paste_preserve_char_boundaries_and_list_state() {
    let mut picker = picker();
    let filtered = picker.filtered.clone();
    picker.show_prompt("Rename", "Choose a name", "aé🦊界", false);
    key(&mut picker, KeyCode::Left);
    key(&mut picker, KeyCode::Backspace);
    assert_eq!(picker.prompt.as_ref().unwrap().input, "aé界");
    key(&mut picker, KeyCode::Delete);
    assert_eq!(picker.prompt.as_ref().unwrap().input, "aé");
    key(&mut picker, KeyCode::Home);
    key(&mut picker, KeyCode::Right);
    assert!(picker.paste_prompt("中\n\r\t🦀"));
    key(&mut picker, KeyCode::Char('ñ'));
    assert_eq!(picker.prompt.as_ref().unwrap().input, "a中🦀ñé");
    key(&mut picker, KeyCode::End);
    key(&mut picker, KeyCode::Char('!'));
    assert_eq!(picker.prompt.as_ref().unwrap().input, "a中🦀ñé!");
    assert_eq!(picker.filter, "");
    assert_eq!(picker.filtered, filtered);
    picker
        .handle_overlay_key(KeyCode::Char('u'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(picker.prompt.as_ref().unwrap().input, "");
    key(&mut picker, KeyCode::Left);
    key(&mut picker, KeyCode::Backspace);
    key(&mut picker, KeyCode::Delete);
    assert_eq!(picker.prompt.as_ref().unwrap().cursor, 0);
}

#[test]
fn submit_keeps_form_for_validation_and_escape_only_cancels_form() {
    let mut picker = picker();
    let view = picker.view;
    let selected = picker.selected;
    picker.show_prompt("Rename", "Name", "name", false);
    assert!(
        matches!(key(&mut picker, KeyCode::Enter), OverlayAction::SubmitPrompt(value) if value == "name")
    );
    assert!(picker.prompt_is_open());
    picker.set_notice("Already exists".into(), true);
    assert!(
        matches!(picker.handle_overlay_key(KeyCode::Enter, KeyModifiers::CONTROL).unwrap(), OverlayAction::SubmitPrompt(value) if value == "name")
    );
    assert!(picker.prompt_is_open());
    assert!(matches!(
        key(&mut picker, KeyCode::Esc),
        OverlayAction::CancelPrompt
    ));
    assert!(!picker.prompt_is_open());
    assert_eq!(picker.view, view);
    assert_eq!(picker.selected, selected);
    assert!(!picker.paste_prompt("ignored"));
    picker.show_prompt("Add", "Name", "", false);
    picker.finish_prompt();
    assert!(!picker.prompt_is_open());
}

#[test]
fn waiting_consumes_typing_paste_and_enter_but_can_cancel() {
    let mut picker = picker();
    picker.show_waiting("Signing in", "Waiting for browser OAuth");
    assert!(picker.prompt_is_open());
    assert!(matches!(
        key(&mut picker, KeyCode::Enter),
        OverlayAction::Continue
    ));
    key(&mut picker, KeyCode::Char('x'));
    assert!(picker.paste_prompt("secret"));
    assert_eq!(picker.prompt.as_ref().unwrap().input, "");
    let text = render(&mut picker, 160, 45);
    assert!(text.contains("Signing in"));
    assert!(!text.contains("Value"));
    assert!(matches!(
        key(&mut picker, KeyCode::Esc),
        OverlayAction::CancelPrompt
    ));
}

#[test]
fn prompt_and_notice_render_with_account_context_and_no_secret_debug_leak() {
    let mut picker = picker();
    picker.show_prompt("Rename login", "Enter a new label", "visible-name", false);
    for (width, height) in [(160, 45), (68, 22), (40, 12)] {
        let text = render(&mut picker, width, height);
        assert!(text.contains("Rename login"));
        assert!(text.contains("visible-name"));
        assert!(text.contains("Enter a new label"));
        assert!(text.contains("Esc: cancel"));
    }
    let text = render(&mut picker, 160, 45);
    assert!(text.contains("Saved accounts"));
    assert!(text.contains("shared"));
    picker.set_notice("Label already exists".into(), true);
    for (width, height) in [(160, 45), (68, 22), (40, 12)] {
        assert!(render(&mut picker, width, height).contains("Label already exists"));
    }
    picker.show_prompt("OAuth", "Paste code", "secret-🦀-value", true);
    let text = render(&mut picker, 160, 45);
    assert!(text.contains("*****"));
    assert!(!text.contains("secret-"));
    assert!(!format!("{picker:?}").contains("secret-"));
    let profile = picker.debug_memory_profile();
    assert!(!profile.to_string().contains("secret-"));
    assert_eq!(profile["prompt_open"], true);
    assert!(profile["prompt_estimate_bytes"].as_u64().unwrap() > 0);
    picker.finish_prompt();
    picker.set_notice("Account saved".into(), false);
    for (width, height) in [(160, 45), (68, 22), (40, 12)] {
        assert!(render(&mut picker, width, height).contains("Account saved"));
    }
}

#[test]
fn prompt_mouse_and_wheel_never_change_underlying_selection() {
    for waiting in [false, true] {
        let mut picker = picker();
        render(&mut picker, 160, 45);
        let list = picker.last_action_list_area.unwrap();
        let selected = picker.selected;
        let filtered = picker.filtered.clone();
        let view = picker.view;
        if waiting {
            picker.show_waiting("Signing in", "Wait for OAuth");
        } else {
            picker.show_prompt("Rename", "Choose a label", "original", false);
        }
        for kind in [
            MouseEventKind::ScrollDown,
            MouseEventKind::ScrollUp,
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
        ] {
            picker.handle_overlay_mouse(MouseEvent {
                kind,
                column: list.x,
                row: list.y + 2,
                modifiers: KeyModifiers::empty(),
            });
            assert_eq!(picker.selected, selected);
            assert_eq!(picker.filtered, filtered);
            assert_eq!(picker.view, view);
            assert!(picker.prompt_is_open());
            assert_eq!(
                picker.prompt.as_ref().unwrap().input,
                if waiting { "" } else { "original" }
            );
        }
    }
}

fn select_replace(picker: &mut AccountPicker) {
    picker.select_account("openai", Some("shared"));
    key(picker, KeyCode::Enter);
    for c in "Replace login".chars() {
        key(picker, KeyCode::Char(c));
    }
    assert_eq!(picker.filtered.len(), 1);
}

#[test]
fn replacement_is_exact_scoped_and_y_only_and_keeps_page_mounted() {
    let mut picker = picker();
    select_replace(&mut picker);
    assert!(matches!(
        key(&mut picker, KeyCode::Enter),
        OverlayAction::Continue
    ));
    let text = render(&mut picker, 160, 45);
    assert!(text.contains("Confirm replacement"));
    assert!(text.contains("old login remains until OAuth succeeds"));
    assert!(text.contains("different identity is allowed"));
    assert!(text.contains("openai account `shared`"));
    assert!(matches!(
        key(&mut picker, KeyCode::Enter),
        OverlayAction::Continue
    ));
    assert!(
        matches!(key(&mut picker, KeyCode::Char('y')), OverlayAction::Execute(AccountPickerCommand::Replace { provider: AccountProviderKind::OpenAi, label }) if label == "shared")
    );
    assert!(matches!(picker.view, AccountPickerView::Account(_)));
}

#[test]
fn replacement_escape_navigation_search_modified_y_and_mouse_disarm() {
    for (code, modifiers) in [
        (KeyCode::Esc, KeyModifiers::empty()),
        (KeyCode::Down, KeyModifiers::empty()),
        (KeyCode::Char('x'), KeyModifiers::empty()),
        (KeyCode::Char('y'), KeyModifiers::CONTROL),
        (KeyCode::Char('Y'), KeyModifiers::SHIFT),
    ] {
        let mut picker = picker();
        select_replace(&mut picker);
        key(&mut picker, KeyCode::Enter);
        assert!(matches!(
            picker.handle_overlay_key(code, modifiers).unwrap(),
            OverlayAction::Continue
        ));
        assert!(picker.removal_confirmation.is_none());
        assert!(!matches!(
            key(&mut picker, KeyCode::Char('y')),
            OverlayAction::Execute(_)
        ));
    }
    let mut picker = picker();
    select_replace(&mut picker);
    key(&mut picker, KeyCode::Enter);
    picker.handle_overlay_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::empty(),
    });
    assert!(picker.removal_confirmation.is_none());
    assert!(!matches!(
        key(&mut picker, KeyCode::Char('y')),
        OverlayAction::Execute(_)
    ));
}
