//! State for the staged model menu opened from the caption below the composer.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Provider,
    Connection,
    Model,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route<'a> {
    pub spec: &'a str,
    pub provider: &'a str,
    pub connection: &'static str,
    pub model: &'a str,
}

impl<'a> Route<'a> {
    pub fn parse(spec: &'a str) -> Self {
        let (route, model) = spec.split_once(':').unwrap_or(("Other", spec));
        let (provider, connection) = if let Some(provider) = route.strip_suffix("-oauth") {
            (provider, "OAuth")
        } else if let Some(provider) = route.strip_suffix("-api") {
            (provider, "API")
        } else {
            (route, "Default")
        };
        Self {
            spec,
            provider,
            connection,
            model,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picker {
    open: bool,
    loading: bool,
    models: Vec<String>,
    current: Option<String>,
    hover: Option<usize>,
    button_hover: bool,
    stage: Stage,
    provider: Option<String>,
    connection: Option<String>,
    /// Reveal progress in thousandths. The panel remains visible while this
    /// runs back to zero, so closing lets the transcript settle together.
    phase: u16,
}

const PHASE_MAX: u16 = 1000;
pub const REVEAL_SECONDS: f32 = 0.18;

impl Picker {
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn is_loading(&self) -> bool {
        self.loading
    }
    pub fn is_visible(&self) -> bool {
        self.open || self.phase > 0
    }
    pub fn phase(&self) -> f64 {
        let t = f64::from(self.phase) / f64::from(PHASE_MAX);
        t * t * (3.0 - 2.0 * t)
    }
    pub fn is_animating(&self) -> bool {
        if self.open {
            self.phase < PHASE_MAX
        } else {
            self.phase > 0
        }
    }
    pub fn advance(&mut self, dt: f32) -> bool {
        let step = (dt / REVEAL_SECONDS * f32::from(PHASE_MAX)).max(1.0) as u16;
        let before = self.phase;
        self.phase = if self.open {
            self.phase.saturating_add(step).min(PHASE_MAX)
        } else {
            self.phase.saturating_sub(step)
        };
        before != self.phase
    }
    /// Vertical displacement for a transcript item in region coordinates.
    /// Rendering and selection both call this so animated ink never drifts away
    /// from its pointer geometry.
    pub fn transcript_shift(
        &self,
        region_height: f64,
        menu_height: f64,
        item_top: f64,
        item_height: f64,
    ) -> f64 {
        if !self.is_visible() {
            return 0.0;
        }
        let amount = (menu_height / 2.0 + crate::layout::MODEL_MENU_GAP) * self.phase();
        if item_top + item_height / 2.0 < region_height / 2.0 {
            -amount
        } else {
            amount
        }
    }
    pub fn stage(&self) -> Stage {
        self.stage
    }
    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }
    pub fn connection(&self) -> Option<&str> {
        self.connection.as_deref()
    }

    pub fn open_loading(&mut self) {
        self.open = true;
        self.loading = true;
        self.stage = Stage::Provider;
        self.provider = None;
        self.connection = None;
        self.hover = Some(0);
    }

    pub fn close(&mut self) {
        self.open = false;
        self.loading = false;
    }

    pub fn set_models(&mut self, models: Vec<String>, current: Option<String>) {
        self.models = models;
        self.current = current;
        self.loading = false;
        self.hover = Some(0);
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

    fn unique_values(&self, value: impl Fn(&Route<'_>) -> String) -> Vec<String> {
        let mut values = Vec::new();
        for spec in &self.models {
            let item = value(&Route::parse(spec));
            if !values.contains(&item) {
                values.push(item);
            }
        }
        values
    }

    pub fn providers(&self) -> Vec<String> {
        self.unique_values(|route| route.provider.to_string())
    }

    pub fn connections(&self) -> Vec<String> {
        let provider = self.provider.as_deref();
        self.unique_values(|route| {
            if Some(route.provider) == provider {
                route.connection.to_string()
            } else {
                String::new()
            }
        })
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
    }

    pub fn visible_models(&self) -> Vec<&str> {
        self.models
            .iter()
            .filter_map(|spec| {
                let route = Route::parse(spec);
                (Some(route.provider) == self.provider.as_deref()
                    && Some(route.connection) == self.connection.as_deref())
                .then_some(spec.as_str())
            })
            .collect()
    }

    pub fn row_labels(&self) -> Vec<String> {
        if self.loading && self.models.is_empty() {
            return vec!["Loading models…".into()];
        }
        let values = match self.stage {
            Stage::Provider => self.providers(),
            Stage::Connection => self.connections(),
            Stage::Model => self
                .visible_models()
                .into_iter()
                .map(|spec| Route::parse(spec).model.to_string())
                .collect(),
        };
        if values.is_empty() {
            vec!["No models available".into()]
        } else {
            values
        }
    }

    /// Advance one stage. Returns a model spec only at the final stage.
    pub fn choose_row(&mut self, index: usize) -> Option<String> {
        if self.loading && self.models.is_empty() {
            return None;
        }
        match self.stage {
            Stage::Provider => {
                self.provider = self.providers().get(index).cloned();
                if self.provider.is_some() {
                    self.stage = Stage::Connection;
                }
            }
            Stage::Connection => {
                self.connection = self.connections().get(index).cloned();
                if self.connection.is_some() {
                    self.stage = Stage::Model;
                }
            }
            Stage::Model => {
                return self
                    .visible_models()
                    .get(index)
                    .map(|value| (*value).to_string());
            }
        }
        self.hover = None;
        None
    }

    pub fn hover(&self) -> Option<usize> {
        self.hover
            .filter(|index| self.open && *index < self.visual_rows())
    }
    pub fn set_hover(&mut self, row: Option<usize>) -> bool {
        let row = row.filter(|index| self.open && *index < self.visual_rows());
        if self.hover == row {
            return false;
        }
        self.hover = row;
        true
    }
    pub fn move_hover(&mut self, delta: isize) {
        let rows = self.visual_rows();
        if rows == 0 {
            return;
        }
        let current = self.hover.unwrap_or(0) as isize;
        self.hover = Some((current + delta).rem_euclid(rows as isize) as usize);
    }
    pub fn choose_hovered(&mut self) -> Option<String> {
        self.choose_row(self.hover.unwrap_or(0))
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
    pub fn visual_rows(&self) -> usize {
        self.row_labels().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_staged_by_provider_connection_and_model() {
        let mut picker = Picker::default();
        picker.open_loading();
        picker.set_models(
            vec![
                "openai-oauth:gpt-5.6".into(),
                "openai-api:gpt-5.6".into(),
                "claude-api:opus".into(),
            ],
            Some("claude-api:opus".into()),
        );
        assert_eq!(picker.row_labels(), ["openai", "claude"]);
        assert_eq!(picker.choose_row(0), None);
        assert_eq!(picker.row_labels(), ["OAuth", "API"]);
        assert_eq!(picker.choose_row(1), None);
        assert_eq!(picker.row_labels(), ["gpt-5.6"]);
        assert_eq!(picker.choose_row(0).as_deref(), Some("openai-api:gpt-5.6"));
    }

    #[test]
    fn a_late_catalog_does_not_reopen_a_dismissed_menu() {
        let mut picker = Picker::default();
        picker.open_loading();
        picker.close();
        picker.set_models(vec!["gpt-5.6".into()], Some("gpt-5.6".into()));
        assert!(!picker.is_open());
    }

    #[test]
    fn reveal_reverses_smoothly_when_closed_mid_animation() {
        let mut picker = Picker::default();
        picker.open_loading();
        assert!(picker.advance(REVEAL_SECONDS / 2.0));
        let halfway = picker.phase();
        assert!(halfway > 0.0 && halfway < 1.0);
        picker.close();
        assert!(picker.is_visible());
        assert!(picker.advance(REVEAL_SECONDS));
        assert!(!picker.is_visible());
    }
}
