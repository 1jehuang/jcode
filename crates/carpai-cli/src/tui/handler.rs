//! TUI Event handler — update() + draw() dispatch
//!
//! Only calls bridge methods, contains no business logic.

use crate::tui::{app::App, event::Event};

impl App {
    /// Handle a TUI event (update phase)
    pub async fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.handle_key(key).await,
            _ => {}
        }
    }

    async fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        // If file tree is focused, handle navigation first
        if self.file_tree.visible {
            match key.code {
                KeyCode::Char('f') if key.modifiers == KeyModifiers::CONTROL => {
                    self.toggle_file_tree();
                    return;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.file_tree.next();
                    return;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.file_tree.previous();
                    return;
                }
                KeyCode::Enter => {
                    if let Some(path) = self.file_tree.selected_path() {
                        let path_str = path.display().to_string();
                        self.input = path_str;
                    }
                    return;
                }
                _ => {}
            }
            return;
        }

        // Help overlay active — any key dismisses
        if self.show_help {
            self.show_help = false;
            return;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => { self.should_quit = true; }
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => { self.toggle_file_tree(); }
            (KeyCode::Char('?'), KeyModifiers::NONE) | (KeyCode::F(1), _) => { self.toggle_help(); }
            (KeyCode::Enter, _) => {
                let input = self.input.clone();
                if !input.is_empty() { self.handle_input(input).await; }
            }
            (KeyCode::Backspace, _) => { self.input.pop(); }
            (KeyCode::Char(c), _) => { self.input.push(c); }
            _ => {}
        }
    }
}
