pub use jcode_tui_account_picker::{
    AccountPicker, AccountPickerCommand, AccountPickerItem, AccountPickerSummary,
    AccountProviderKind, OverlayAction,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    fn buffer_to_text(buffer: &ratatui::buffer::Buffer) -> String {
        let area = buffer.area;
        let mut out = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn text_contains_wrapped(rendered: &str, expected: &str) -> bool {
        if rendered.contains(expected) {
            return true;
        }
        let tokens = expected.split_whitespace().collect::<Vec<_>>();
        if tokens.is_empty() {
            return true;
        }
        let mut start = 0;
        for token in tokens {
            let Some(offset) = rendered[start..].find(token) else {
                return false;
            };
            start += offset + token.len();
        }
        true
    }

    #[test]
    fn account_picker_catalog_state_space_renders_and_executes_every_provider_action() {
        let providers = crate::provider_catalog::login_providers();
        assert!(
            !providers.is_empty(),
            "login provider catalog should not be empty"
        );

        for provider in providers.iter().copied() {
            let command = format!("/account {} login", provider.id);
            let title = format!("Login / refresh {}", provider.display_name);
            let subtitle = format!("state-space account action for {}", provider.id);
            let mut picker = AccountPicker::with_summary(
                " Accounts ",
                vec![AccountPickerItem::action(
                    provider.id,
                    provider.display_name,
                    title.clone(),
                    subtitle.clone(),
                    AccountPickerCommand::SubmitInput(command.clone()),
                )],
                AccountPickerSummary {
                    provider_count: 1,
                    setup_count: 1,
                    default_provider: Some("auto".to_string()),
                    default_model: Some("provider default".to_string()),
                    ..AccountPickerSummary::default()
                },
            );

            let backend = TestBackend::new(140, 46);
            let mut terminal = Terminal::new(backend).expect("failed to create terminal");
            terminal
                .draw(|frame| picker.render(frame))
                .expect("draw failed");
            let text = buffer_to_text(terminal.backend().buffer());

            for expected in [
                provider.display_name,
                provider.id,
                title.as_str(),
                subtitle.as_str(),
            ] {
                assert!(
                    text_contains_wrapped(&text, expected),
                    "account picker missing {expected:?} for provider={}; rendered:\n{text}",
                    provider.id
                );
            }

            match picker
                .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                .expect("enter should be handled")
            {
                OverlayAction::Execute(AccountPickerCommand::SubmitInput(input)) => {
                    assert_eq!(input, command)
                }
                _ => panic!(
                    "Enter should execute account command for provider={}",
                    provider.id
                ),
            }
        }
    }
}
