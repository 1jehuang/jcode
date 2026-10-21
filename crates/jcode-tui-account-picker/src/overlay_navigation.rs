use super::*;

impl AccountPicker {
    /// Keep the full command catalog, but expose saved accounts before their actions.
    pub fn with_account_menus(mut self) -> Self {
        self.manager_scope.get_or_insert(None);
        let replacements: Vec<_> = self.items.iter().filter_map(|item| {
            let AccountPickerCommand::Switch { provider, label } = &item.command else {
                return None;
            };
            Some(AccountPickerItem::action(
                &item.provider_id, &item.provider_label, "Replace login",
                "Replace this saved login after OAuth succeeds; a different identity is allowed",
                AccountPickerCommand::Replace { provider: provider.clone(), label: label.clone() },
            ))
        }).collect();
        self.items.extend(replacements);
        let defaults: Vec<_> = self
            .items
            .iter()
            .filter_map(|item| {
                let AccountPickerCommand::Switch { provider, label } = &item.command else {
                    return None;
                };
                Some(AccountPickerItem::action(
                    &item.provider_id,
                    &item.provider_label,
                    "Make default",
                    "Use this account for new and unpinned windows",
                    AccountPickerCommand::SetDefault {
                        provider: provider.clone(),
                        label: label.clone(),
                    },
                ))
            })
            .collect();
        self.items.extend(defaults);
        self.items.push(AccountPickerItem::action(
            "account-flow",
            "Accounts",
            "Provider settings",
            "Provider sign-in, diagnostics, and global defaults",
            AccountPickerCommand::OpenProviderSettings,
        ));
        self.view = AccountPickerView::Accounts;
        self.selected = 0;
        self.apply_filter();
        self
    }

    pub(super) fn view_name(&self) -> &'static str {
        match self.view {
            AccountPickerView::Flat => "flat",
            AccountPickerView::Accounts => "accounts",
            AccountPickerView::Account(_) => "account",
            AccountPickerView::ProviderSettings => "provider_settings",
        }
    }

    pub(super) fn item_in_view(&self, index: usize) -> bool {
        let item = &self.items[index];
        match self.view {
            AccountPickerView::Flat => true,
            AccountPickerView::Accounts => {
                action_section(item) == ActionSection::Switch
                    || matches!(
                        item.command,
                        AccountPickerCommand::OpenAddReplaceFlow { .. }
                            | AccountPickerCommand::OpenProviderSettings
                    )
            }
            AccountPickerView::Account(account) => {
                let Some(target) = account_target(&self.items[account].command) else {
                    return false;
                };
                account_target(&item.command) == Some(target)
            }
            AccountPickerView::ProviderSettings => {
                account_target(&item.command).is_none()
                    && action_section(item) != ActionSection::Switch
                    && !matches!(
                        item.command,
                        AccountPickerCommand::OpenAddReplaceFlow { .. }
                            | AccountPickerCommand::OpenProviderSettings
                    )
            }
        }
    }

    pub(super) fn open_selected_menu(&mut self) -> bool {
        if self.view != AccountPickerView::Accounts {
            return false;
        }
        let Some(index) = self.filtered.get(self.selected).copied() else {
            return false;
        };
        let view = match self.items[index].command {
            AccountPickerCommand::Switch { .. } => AccountPickerView::Account(index),
            AccountPickerCommand::OpenProviderSettings => AccountPickerView::ProviderSettings,
            _ => return false,
        };
        self.account_list_position = Some((std::mem::take(&mut self.filter), self.selected));
        self.view = view;
        self.selected = 0;
        self.apply_filter();
        true
    }

    pub(super) fn return_to_account_list(&mut self) -> bool {
        let Some((filter, selected)) = self.account_list_position.take() else {
            return false;
        };
        self.view = AccountPickerView::Accounts;
        self.filter = filter;
        self.selected = selected;
        self.apply_filter();
        true
    }

    pub(super) fn navigation_title(&self) -> String {
        match self.view {
            AccountPickerView::Account(index) => format!(
                "Accounts > {} > {}",
                self.items[index].provider_label,
                compact_item_title(&self.items[index])
            ),
            AccountPickerView::ProviderSettings => "Accounts > Provider settings".to_string(),
            _ => self.title.clone(),
        }
    }

    pub(super) fn render_account_action_details(
        &self,
        frame: &mut Frame,
        area: Rect,
        index: usize,
        action: &AccountPickerItem,
    ) {
        let account = &self.items[index];
        let mut lines = vec![
            Line::styled(
                compact_item_title(account),
                Style::default().fg(Color::White).bold(),
            ),
            Line::styled(
                action.title.clone(),
                Style::default().fg(Color::White).bold(),
            ),
            Line::from("Enter: run action. Esc: back to accounts."),
            Line::from(""),
        ];
        for (label, value) in &account.details {
            lines.push(Line::styled(
                label.clone(),
                Style::default().fg(Color::White).bold(),
            ));
            lines.extend(
                value
                    .lines()
                    .map(|line| Line::styled(line.to_owned(), Style::default().fg(MUTED))),
            );
            lines.push(Line::from(""));
        }
        lines.push(Line::styled(
            account.subtitle.clone(),
            Style::default().fg(MUTED),
        ));
        lines.push(Line::from(action_kind_help(&action.command)));
        lines.push(Line::from(command_preview(&action.command)));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }
}

fn account_target(command: &AccountPickerCommand) -> Option<(&AccountProviderKind, &str)> {
    match command {
        AccountPickerCommand::Switch { provider, label }
        | AccountPickerCommand::SetDefault { provider, label }
        | AccountPickerCommand::Replace { provider, label }
        | AccountPickerCommand::Login { provider, label }
        | AccountPickerCommand::Rename { provider, label }
        | AccountPickerCommand::Remove { provider, label } => Some((provider, label)),
        _ => None,
    }
}

#[cfg(test)]
#[path = "overlay_navigation_tests.rs"]
mod tests;
