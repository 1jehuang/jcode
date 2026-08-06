//! Keyboard, pointer, harness, and animation behavior for the inline model picker.

use crate::{App, harness};
use winit::keyboard::{Key, NamedKey};

impl App {
    /// Toggle the inline catalog from its Ctrl+M shortcut. Opening requests a
    /// fresh SDK catalog and closes other page chrome before beginning the
    /// transcript reveal.
    pub(crate) fn toggle_model_picker(&mut self) {
        if self.model.model_picker.is_open() {
            self.model.model_picker.close();
            self.request_redraw();
            return;
        }
        let Some((_, outgoing)) = self.harness.as_ref() else {
            self.model.set_notice("not connected: cannot list models");
            self.request_redraw();
            return;
        };
        if outgoing.send(harness::Command::ListModels).is_err() {
            self.model.set_notice("not connected: cannot list models");
            self.request_redraw();
            return;
        }
        self.model.panel.close();
        self.model.model_picker.open_loading();
        self.request_redraw();
    }

    fn commit_model_picker_choice(&mut self, model: String) {
        self.model.model_picker.close();
        if let Some((_, outgoing)) = self.harness.as_ref() {
            if outgoing.send(harness::Command::SetModel(model)).is_err() {
                self.model.set_notice("not connected: cannot change model");
            }
        } else {
            self.model.set_notice("not connected: cannot change model");
        }
    }

    /// While visible, the inline chooser owns navigation keys so moving through
    /// models cannot also move the composer caret or recall prompt history.
    pub(crate) fn model_picker_keydown(&mut self, key: &Key) -> bool {
        match key {
            Key::Named(NamedKey::Escape) => self.model.model_picker.close(),
            Key::Named(NamedKey::ArrowUp) => self.model.model_picker.move_hover(-1),
            Key::Named(NamedKey::ArrowDown) => self.model.model_picker.move_hover(1),
            Key::Named(NamedKey::Enter) => {
                if let Some(model) = self.model.model_picker.choose_hovered() {
                    self.commit_model_picker_choice(model);
                }
            }
            Key::Character(text)
                if text.eq_ignore_ascii_case("m") && self.modifiers.control_key() =>
            {
                self.model.model_picker.close();
            }
            Key::Character(text)
                if text.eq_ignore_ascii_case("m") && self.modifiers.super_key() =>
            {
                self.model.model_picker.close();
            }
            _ => return true,
        }
        self.request_redraw();
        true
    }

    pub(crate) fn tick_model_picker(&mut self, dt: f32) {
        if self.model.model_picker.advance(dt) {
            self.request_redraw();
        }
    }

    /// Let the caption or its open menu consume a press before the composer and
    /// transcript see it. Like the settings panel, dismiss clicks are consumed
    /// so closing a menu cannot also move the caret underneath it.
    pub(crate) fn model_picker_press(&mut self, x: f64, y: f64) -> bool {
        if self.model.model_picker.is_open() {
            let rows = self.model.model_picker.visual_rows();
            if let Some(index) = self.frame.model_menu_row_at(rows, x, y) {
                if let Some(model) = self.model.model_picker.choose_row(index) {
                    self.commit_model_picker_choice(model);
                }
                self.request_redraw();
                return true;
            }
            self.model.model_picker.close();
            self.request_redraw();
            return true;
        }

        false
    }

    /// Track both the caption button and the rows in its menu. Returns whether
    /// painting state changed.
    pub(crate) fn model_picker_hover(&mut self, x: f64, y: f64) -> bool {
        let mut changed = self.model.model_picker.set_button_hover(false);
        let row = self
            .model
            .model_picker
            .is_open()
            .then(|| {
                self.frame
                    .model_menu_row_at(self.model.model_picker.visual_rows(), x, y)
            })
            .flatten();
        changed |= self.model.model_picker.set_hover(row);
        changed
    }
}
