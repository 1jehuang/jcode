use super::*;

#[test]
fn parse_account_doctor_subcommands() {
    assert!(matches!(
        parse_account_command("/accounts doctor"),
        Some(Ok(AccountCommand::Doctor { provider_id: None }))
    ));
    assert!(matches!(
        parse_account_command("/accounts openai doctor"),
        Some(Ok(AccountCommand::Doctor { provider_id: Some(provider_id) })) if provider_id == "openai"
    ));
}

#[test]
fn parse_claude_code_instance_commands() {
    assert!(matches!(
        parse_account_command("/accounts claude-code switch personal"),
        Some(Ok(AccountCommand::UseInWindow { provider_id, label }))
            if provider_id == "claude-code" && label == "personal"
    ));
    assert!(matches!(
        parse_account_command("/accounts claude-code personal"),
        Some(Ok(AccountCommand::UseInWindow { provider_id, label }))
            if provider_id == "claude-code" && label == "personal"
    ));
    assert!(matches!(
        parse_account_command("/accounts claude-code"),
        Some(Ok(AccountCommand::OpenOverlay { provider_filter: Some(filter) }))
            if filter == "claude-code"
    ));
    assert!(matches!(
        parse_account_command("/accounts claude-code unpin"),
        Some(Ok(AccountCommand::Unpin { provider_id: Some(provider_id) }))
            if provider_id == "claude-code"
    ));
    assert!(matches!(
        parse_account_command("/accounts unpin claude-code"),
        Some(Ok(AccountCommand::Unpin { provider_id: Some(provider_id) }))
            if provider_id == "claude-code"
    ));
    assert!(matches!(
        parse_account_command("/accounts claude-code default personal"),
        Some(Err(_))
    ));
}

#[test]
fn parse_native_jcode_account_actions() {
    assert!(matches!(
        parse_account_command("/accounts jcode login"),
        Some(Ok(AccountCommand::Login { provider_id })) if provider_id == "jcode"
    ));
    assert!(matches!(
        parse_account_command("/accounts jcode status"),
        Some(Ok(AccountCommand::JcodeStatus))
    ));
    assert!(matches!(
        parse_account_command("/accounts jcode manage"),
        Some(Ok(AccountCommand::JcodeManage))
    ));
    assert!(matches!(
        parse_account_command("/accounts jcode logout"),
        Some(Ok(AccountCommand::JcodeLogout))
    ));
}

fn parsed(input: &str) -> String {
    format!("{:?}", parse_account_command(input))
}

#[test]
fn parse_account_switch_pins_this_window() {
    for input in [
        "/accounts switch claude-fox",
        "/accounts use claude-fox",
        "/accounts claude switch claude-fox",
        "/accounts claude claude-fox",
        "/accounts claude-fox",
    ] {
        let out = parsed(input);
        assert!(out.contains("UseInWindow"), "{input} -> {out}");
        assert!(out.contains("claude-fox"), "{input} -> {out}");
    }
    let out = parsed("/accounts openai switch openai-otter");
    assert!(
        out.contains("UseInWindow") && out.contains("\"openai\""),
        "{out}"
    );
}

#[test]
fn parse_account_default_label_sets_default_not_provider_fallthrough() {
    let out = parsed("/accounts default claude-fox");
    assert!(out.contains("SetDefault {"), "{out}");
    assert!(out.contains("claude-fox"), "{out}");
    assert!(!out.contains("UseInWindow"), "{out}");
    let out = parsed("/accounts openai default openai-fox");
    assert!(
        out.contains("SetDefault {") && out.contains("openai-fox"),
        "{out}"
    );
    // Existing global-default commands keep their meaning.
    assert!(parsed("/accounts default-provider claude").contains("SetDefaultProvider"));
    assert!(parsed("/accounts default-model gpt-5").contains("SetDefaultModel"));
    assert!(parsed("/accounts default").contains("Err"));
}

#[test]
fn parse_account_unpin() {
    let out = parsed("/accounts unpin");
    assert!(out.contains("Unpin { provider_id: None }"), "{out}");
    let out = parsed("/accounts unpin openai");
    assert!(
        out.contains("Unpin { provider_id: Some(\"openai\") }"),
        "{out}"
    );
    let out = parsed("/accounts claude unpin");
    assert!(
        out.contains("Unpin { provider_id: Some(\"claude\") }"),
        "{out}"
    );
}

#[test]
fn parse_account_failover_modes() {
    assert!(parsed("/accounts failover on").contains("Failover(On)"));
    assert!(parsed("/accounts failover off").contains("Failover(Off)"));
    assert!(parsed("/accounts failover default").contains("Failover(Default)"));
    assert!(parsed("/accounts failover status").contains("Failover(Status)"));
    assert!(parsed("/accounts failover").contains("Failover(Status)"));
    assert!(parsed("/accounts failover sideways").contains("Err"));
}

#[test]
fn parse_account_command_requires_a_token_boundary() {
    for removed in [
        "/account",
        "/account list",
        "/account openai add",
        "/accounts-extra",
    ] {
        assert!(parse_account_command(removed).is_none(), "{removed}");
    }
    assert!(
        parse_account_command("/accounting").is_none(),
        "/accounting must not be parsed as /accounts with label `ing`"
    );
    assert!(matches!(
        parse_account_command("/accounts"),
        Some(Ok(AccountCommand::OpenOverlay {
            provider_filter: None
        }))
    ));
}

#[test]
fn parse_accounts_rename_and_reauthenticate_keep_the_selected_target() {
    for provider in ["claude", "openai"] {
        assert!(matches!(
            parse_account_command(&format!("/accounts {provider} rename work New account name")),
            Some(Ok(AccountCommand::Rename { provider_id, label, new_label: Some(name) }))
                if provider_id == provider && label == "work" && name == "New account name"
        ));
        assert!(matches!(
            parse_account_command(&format!("/accounts {provider} rename work")),
            Some(Ok(AccountCommand::Rename { provider_id, label, new_label: None }))
                if provider_id == provider && label == "work"
        ));
        assert!(matches!(
            parse_account_command(&format!("/accounts {provider} replace Work account")),
            Some(Ok(AccountCommand::Replace { provider_id, label }))
                if provider_id == provider && label == "Work account"
        ));
        assert!(matches!(
            parse_account_command(&format!("/accounts {provider} replace")),
            Some(Err(_))
        ));
        for action in ["reauth", "reauthenticate", "refresh"] {
            assert!(matches!(
                parse_account_command(&format!("/accounts {provider} {action} Work account")),
                Some(Ok(AccountCommand::Reauthenticate { provider_id, label }))
                    if provider_id == provider && label == "Work account"
            ));
            assert!(matches!(
                parse_account_command(&format!("/accounts {provider} {action}")),
                Some(Err(_))
            ));
        }
        assert!(matches!(
            parse_account_command(&format!("/accounts {provider} rename")),
            Some(Err(_))
        ));
    }
}

#[test]
fn account_picker_management_actions_preserve_labels_with_spaces() {
    use crate::tui::account_picker::{AccountPickerCommand, AccountProviderKind};
    assert!(matches!(
        account_command_from_picker(&AccountPickerCommand::Rename {
            provider: AccountProviderKind::OpenAi,
            label: "Work account".to_string(),
        }),
        Some(AccountCommand::Rename { provider_id, label, new_label: None })
            if provider_id == "openai" && label == "Work account"
    ));
    assert!(matches!(
        account_command_from_picker(&AccountPickerCommand::Replace {
            provider: AccountProviderKind::Anthropic,
            label: "Work account".to_string(),
        }),
        Some(AccountCommand::Replace { provider_id, label })
            if provider_id == "claude" && label == "Work account"
    ));
    assert!(matches!(
        account_command_from_picker(&AccountPickerCommand::Login {
            provider: AccountProviderKind::Anthropic,
            label: "Work account".to_string(),
        }),
        Some(AccountCommand::Reauthenticate { provider_id, label })
            if provider_id == "claude" && label == "Work account"
    ));
}

#[test]
fn render_auth_doctor_markdown_includes_recovery_steps() {
    let _guard = crate::storage::lock_test_env();
    let markdown = render_auth_doctor_markdown(Some("openai"));
    assert!(markdown.contains("OpenAI (openai)"));
    assert!(markdown.contains("Next steps"));
    assert!(markdown.contains("jcode login --provider openai"));
    assert!(markdown.contains("Review current state: jcode auth status --json"));
}
