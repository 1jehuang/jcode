use super::helpers::slash_command_preview_filter;
use super::preview_request::InlinePickerPreviewRequest;
use super::*;
use crate::tui::PickerKind;
use crossterm::event::{KeyCode, KeyModifiers};

impl App {
    pub(crate) fn model_picker_preview_filter(input: &str) -> Option<String> {
        slash_command_preview_filter(input, &["/model", "/models"])
    }

    pub(crate) fn login_picker_preview_filter(input: &str) -> Option<String> {
        slash_command_preview_filter(input, &["/login"])
    }

    fn account_picker_preview_request(&self, input: &str) -> Option<InlinePickerPreviewRequest> {
        let trimmed = input.trim_start();
        let rest = trimmed
            .strip_prefix("/account")
            .or_else(|| trimmed.strip_prefix("/accounts"))?;

        if rest.is_empty() {
            return Some(InlinePickerPreviewRequest::Account {
                provider_filter: None,
                filter: String::new(),
            });
        }

        if !rest
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false)
        {
            return None;
        }

        let rest = rest.trim_start();
        if rest.is_empty() {
            return Some(InlinePickerPreviewRequest::Account {
                provider_filter: None,
                filter: String::new(),
            });
        }

        let mut parts = rest.split_whitespace();
        let first = parts.next()?;
        let remainder = parts.collect::<Vec<_>>().join(" ");
        let remainder = remainder.trim();

        match first {
            "switch" | "use" | "add" | "login" | "remove" | "rm" | "delete"
            | "default-provider" | "default-model" => return None,
            "list" | "ls" => {
                return Some(InlinePickerPreviewRequest::Account {
                    provider_filter: None,
                    filter: String::new(),
                });
            }
            _ => {}
        }

        let provider = crate::provider_catalog::resolve_login_provider(first);
        let provider_filter =
            provider.and_then(|provider| self.inline_account_picker_scope_key(Some(provider.id)));

        if provider.is_some() && provider_filter.is_none() {
            return None;
        }

        if let Some(provider_filter) = provider_filter {
            if remainder.is_empty() {
                return Some(InlinePickerPreviewRequest::Account {
                    provider_filter: Some(provider_filter),
                    filter: String::new(),
                });
            }

            let subcommand = remainder.split_whitespace().next().unwrap_or_default();
            match subcommand {
                "list" | "ls" => Some(InlinePickerPreviewRequest::Account {
                    provider_filter: Some(provider_filter),
                    filter: String::new(),
                }),
                "settings" | "login" | "add" | "switch" | "use" | "remove" | "rm" | "delete"
                | "transport" | "effort" | "fast" | "premium" | "api-base" | "api-key-name"
                | "env-file" | "default-model" => None,
                _ => Some(InlinePickerPreviewRequest::Account {
                    provider_filter: Some(provider_filter),
                    filter: remainder.to_string(),
                }),
            }
        } else {
            Some(InlinePickerPreviewRequest::Account {
                provider_filter: None,
                filter: rest.to_string(),
            })
        }
    }

    fn inline_picker_preview_request(&self, input: &str) -> Option<InlinePickerPreviewRequest> {
        Self::model_picker_preview_filter(input)
            .map(|filter| InlinePickerPreviewRequest::Model { filter })
            .or_else(|| {
                Self::login_picker_preview_filter(input)
                    .map(|filter| InlinePickerPreviewRequest::Login { filter })
            })
            .or_else(|| self.account_picker_preview_request(input))
    }

    pub(crate) fn sync_model_picker_preview_from_input(&mut self) {
        let Some(request) = self.inline_picker_preview_request(&self.input) else {
            if self
                .inline_interactive_state
                .as_ref()
                .map(|picker| picker.preview)
                .unwrap_or(false)
            {
                self.inline_interactive_state = None;
            }
            return;
        };

        let should_open = self
            .inline_interactive_state
            .as_ref()
            .map(|picker| !request.matches_picker(self, picker))
            .unwrap_or(true);

        if should_open {
            let saved_input = self.input.clone();
            let saved_cursor = self.cursor_pos;
            request.open(self);
            if let Some(ref mut picker) = self.inline_interactive_state {
                picker.preview = true;
            }
            // Preview must not steal the user's command input.
            self.input = saved_input;
            self.cursor_pos = saved_cursor;
        }

        if let Some(ref mut picker) = self.inline_interactive_state
            && picker.preview
        {
            picker.filter = request.filter().to_string();
            Self::apply_inline_interactive_filter(picker);
        }
    }

    pub(crate) fn activate_picker_from_preview(&mut self) -> bool {
        if !self
            .inline_interactive_state
            .as_ref()
            .map(|picker| picker.preview)
            .unwrap_or(false)
        {
            return false;
        }

        if let Some(ref mut picker) = self.inline_interactive_state {
            picker.preview = false;
        }
        if self
            .inline_interactive_state
            .as_ref()
            .map(|picker| picker.kind == PickerKind::Usage)
            .unwrap_or(false)
        {
            if let Some(ref mut picker) = self.inline_interactive_state {
                picker.column = 0;
            }
            self.input.clear();
            self.cursor_pos = 0;
            return true;
        }
        self.input.clear();
        self.cursor_pos = 0;
        let _ = self.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE);
        true
    }
}
