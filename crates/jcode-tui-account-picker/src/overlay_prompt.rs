use super::*;

/// Debug deliberately omits all prompt text, including OAuth codes.
#[derive(Clone)]
pub(super) struct AccountPrompt {
    title: String,
    body: String,
    input: String,
    cursor: usize,
    sensitive: bool,
    waiting: bool,
}

impl std::fmt::Debug for AccountPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountPrompt")
            .field("sensitive", &self.sensitive)
            .field("waiting", &self.waiting)
            .finish_non_exhaustive()
    }
}

impl AccountPicker {
    /// Mark a flat chooser or account menu as part of the persistent manager.
    pub fn with_manager_scope(mut self, scope: Option<String>) -> Self {
        self.manager_scope = Some(scope);
        self
    }

    pub fn manager_scope(&self) -> Option<Option<String>> {
        self.manager_scope.clone()
    }

    pub fn is_manager_subpage(&self) -> bool {
        self.manager_scope.is_some() && self.view == AccountPickerView::Flat
    }

    /// Submission leaves this form mounted so validation errors can be retried.
    pub fn show_prompt(&mut self, title: &str, body: &str, initial: &str, sensitive: bool) {
        self.removal_confirmation = None;
        self.notice = None;
        let input = single_line(initial);
        let cursor = input.len();
        self.prompt = Some(AccountPrompt {
            title: title.into(),
            body: body.into(),
            input,
            cursor,
            sensitive,
            waiting: false,
        });
    }

    pub fn show_waiting(&mut self, title: &str, body: &str) {
        self.show_prompt(title, body, "", false);
        if let Some(prompt) = &mut self.prompt {
            prompt.waiting = true;
        }
    }

    pub fn finish_prompt(&mut self) {
        self.prompt = None;
    }

    pub fn prompt_is_open(&self) -> bool {
        self.prompt.is_some()
    }

    pub fn paste_prompt(&mut self, text: &str) -> bool {
        let Some(prompt) = &mut self.prompt else {
            return false;
        };
        if !prompt.waiting {
            let text = single_line(text);
            prompt.input.insert_str(prompt.cursor, &text);
            prompt.cursor += text.len();
        }
        true
    }

    pub fn set_notice(&mut self, message: String, error: bool) {
        self.notice = Some((message, error));
    }

    pub(super) fn prompt_estimate_bytes(&self) -> usize {
        self.prompt.as_ref().map_or(0, |p| {
            p.title.capacity() + p.body.capacity() + p.input.capacity()
        }) + self.notice.as_ref().map_or(0, |(text, _)| text.capacity())
            + self
                .manager_scope
                .as_ref()
                .and_then(|s| s.as_ref())
                .map_or(0, String::capacity)
    }

    pub(super) fn handle_prompt_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> OverlayAction {
        if code == KeyCode::Esc
            || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
        {
            self.finish_prompt();
            return OverlayAction::CancelPrompt;
        }
        let prompt = self.prompt.as_mut().expect("prompt checked by caller");
        if prompt.waiting {
            return OverlayAction::Continue;
        }
        match code {
            KeyCode::Enter => return OverlayAction::SubmitPrompt(prompt.input.clone()),
            KeyCode::Home => prompt.cursor = 0,
            KeyCode::End => prompt.cursor = prompt.input.len(),
            KeyCode::Left => prompt.cursor = previous_boundary(&prompt.input, prompt.cursor),
            KeyCode::Right => prompt.cursor = next_boundary(&prompt.input, prompt.cursor),
            KeyCode::Backspace => {
                let start = previous_boundary(&prompt.input, prompt.cursor);
                prompt.input.drain(start..prompt.cursor);
                prompt.cursor = start;
            }
            KeyCode::Delete => {
                let end = next_boundary(&prompt.input, prompt.cursor);
                prompt.input.drain(prompt.cursor..end);
            }
            KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.input.clear();
                prompt.cursor = 0;
            }
            KeyCode::Char(c)
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !c.is_control() =>
            {
                prompt.input.insert(prompt.cursor, c);
                prompt.cursor += c.len_utf8();
            }
            _ => {}
        }
        OverlayAction::Continue
    }

    pub(super) fn render_prompt_and_notice(&self, frame: &mut Frame, area: Rect) {
        let Some(prompt) = &self.prompt else {
            if let Some((message, error)) = &self.notice {
                let height = area.height.min(3);
                let notice_area = Rect::new(
                    area.x,
                    area.bottom().saturating_sub(height),
                    area.width,
                    height,
                );
                frame.render_widget(Clear, notice_area);
                frame.render_widget(
                    Paragraph::new(message.as_str())
                        .wrap(Wrap { trim: false })
                        .block(Block::bordered().title(if *error { " Error " } else { " Status " }))
                        .style(Style::default().bg(PANEL_BG).fg(if *error {
                            Color::Red
                        } else {
                            Color::Green
                        })),
                    notice_area,
                );
            }
            return;
        };
        // Keep the account list visible on the left, including at compact sizes.
        let width = if area.width >= 50 {
            area.width * 2 / 3
        } else {
            area.width.saturating_sub(10)
        };
        let panel = Rect::new(
            area.right().saturating_sub(width),
            area.y,
            width,
            area.height,
        );
        frame.render_widget(Clear, panel);
        let block = Block::bordered()
            .title(format!(" {} ", prompt.title))
            .style(Style::default().bg(PANEL_BG).fg(Color::White))
            .border_style(Style::default().fg(PANEL_BORDER_ACTIVE));
        let inner = block.inner(panel);
        frame.render_widget(block, panel);
        let help = if prompt.waiting {
            "Esc: cancel"
        } else {
            "Enter / Ctrl+Enter: submit | Esc: cancel"
        };
        let help_height = (help.len() as u16).div_ceil(inner.width.max(1));
        let footer_height = help_height + if self.notice.is_some() { 3 } else { 0 };
        let rows = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(if prompt.waiting { 0 } else { 3 }),
            Constraint::Length(footer_height),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(prompt.body.as_str()).wrap(Wrap { trim: false }),
            rows[0],
        );
        if !prompt.waiting {
            let display = if prompt.sensitive {
                "*".repeat(prompt.input.chars().count())
            } else {
                prompt.input.clone()
            };
            let cursor_cells = if prompt.sensitive {
                prompt.input[..prompt.cursor].chars().count()
            } else {
                Line::from(&prompt.input[..prompt.cursor]).width()
            };
            let available = rows[1].width.saturating_sub(2) as usize;
            let scroll = cursor_cells.saturating_sub(available.saturating_sub(1));
            frame.render_widget(
                Paragraph::new(display)
                    .scroll((0, scroll.min(u16::MAX as usize) as u16))
                    .block(Block::bordered().title(" Value ")),
                rows[1],
            );
            if available > 0 && rows[1].height >= 3 {
                frame.set_cursor_position((
                    rows[1].x + 1 + (cursor_cells - scroll) as u16,
                    rows[1].y + 1,
                ));
            }
        }
        let mut lines = vec![Line::from(help)];
        if let Some((message, error)) = &self.notice {
            lines.push(Line::styled(
                message.clone(),
                Style::default().fg(if *error { Color::Red } else { Color::Green }),
            ));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), rows[2]);
    }
}

fn single_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn previous_boundary(input: &str, cursor: usize) -> usize {
    input[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(input: &str, cursor: usize) -> usize {
    input[cursor..]
        .chars()
        .next()
        .map_or(cursor, |c| cursor + c.len_utf8())
}

#[cfg(test)]
#[path = "overlay_prompt_tests.rs"]
mod tests;
