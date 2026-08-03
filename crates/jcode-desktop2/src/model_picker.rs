//! State for the model menu opened from the caption below the composer.
//!
//! Geometry and painting live beside the rest of the desktop chrome. This
//! module only owns the catalog returned by the SDK and transient pointer
//! state, which keeps the request/response path testable without a window.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picker {
    open: bool,
    loading: bool,
    models: Vec<String>,
    current: Option<String>,
    hover: Option<usize>,
    button_hover: bool,
}

impl Picker {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Open immediately while a fresh SDK catalog is requested. An old catalog
    /// remains visible during refresh so reopening the menu never flashes empty.
    pub fn open_loading(&mut self) {
        self.open = true;
        self.loading = true;
        self.hover = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.loading = false;
        self.hover = None;
    }

    /// Adopt one `list_models` result without reopening a menu the user already
    /// dismissed while the request was in flight.
    pub fn set_models(&mut self, models: Vec<String>, current: Option<String>) {
        self.models = models;
        self.current = current;
        self.loading = false;
        self.hover = None;
    }

    pub fn models(&self) -> &[String] {
        &self.models
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn mark_selected(&mut self, model: String) {
        self.current = Some(model);
    }

    pub fn hover(&self) -> Option<usize> {
        self.hover
            .filter(|index| self.open && *index < self.models.len())
    }

    pub fn set_hover(&mut self, row: Option<usize>) -> bool {
        let row = row.filter(|index| self.open && *index < self.models.len());
        if self.hover == row {
            return false;
        }
        self.hover = row;
        true
    }

    pub fn button_hover(&self) -> bool {
        self.button_hover
    }

    pub fn set_button_hover(&mut self, hovered: bool) -> bool {
        if self.button_hover == hovered {
            return false;
        }
        self.button_hover = hovered;
        true
    }

    /// The menu always has one visual row while loading or when the catalog is
    /// empty, but only actual SDK models are selectable.
    pub fn visual_rows(&self) -> usize {
        self.models.len().max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_results_are_preserved_in_order_with_the_current_model() {
        let mut picker = Picker::default();
        picker.open_loading();
        picker.set_models(
            vec!["openai-oauth:gpt-5.6".into(), "claude-api:opus".into()],
            Some("claude-api:opus".into()),
        );
        assert_eq!(picker.models(), ["openai-oauth:gpt-5.6", "claude-api:opus"]);
        assert_eq!(picker.current(), Some("claude-api:opus"));
        assert!(!picker.is_loading());
        assert!(picker.is_open());
    }

    #[test]
    fn a_late_catalog_does_not_reopen_a_dismissed_menu() {
        let mut picker = Picker::default();
        picker.open_loading();
        picker.close();
        picker.set_models(vec!["gpt-5.6".into()], Some("gpt-5.6".into()));
        assert!(!picker.is_open());
        assert_eq!(picker.models(), ["gpt-5.6"]);
    }

    #[test]
    fn hover_never_points_past_the_sdk_catalog() {
        let mut picker = Picker::default();
        picker.open_loading();
        picker.set_models(vec!["a".into()], None);
        assert!(picker.set_hover(Some(4)) == false);
        assert!(picker.set_hover(Some(0)));
        assert_eq!(picker.hover(), Some(0));
        picker.close();
        assert_eq!(picker.hover(), None);
    }
}
