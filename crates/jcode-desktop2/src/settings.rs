//! User settings, and the little panel the gear opens.
//!
//! Three things a user actually wants to change while the app is running:
//! which palette it wears, how much of the model's thinking the transcript
//! keeps, and whether the hero animates. Everything else the app decides for
//! itself, because a settings panel that mirrors every constant is a way of
//! refusing to make a decision.
//!
//! The state is pure and file-backed in the same line-oriented format as
//! `window_state`: no dependency, and a corrupt file degrades to defaults
//! rather than failing to start. Panel geometry lives in `layout`, drawing in
//! `scene`, so this module stays testable without a GPU.

use crate::reasoning::ReasoningMode;
use crate::theme::ThemeMode;
use std::path::PathBuf;

/// One toggleable setting: what it is called, and what it currently says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Theme,
    Reasoning,
    Motion,
}

/// Every row, in the order the panel draws them.
pub const ROWS: &[Row] = &[Row::Theme, Row::Reasoning, Row::Motion];

impl Row {
    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Reasoning => "thinking",
            Self::Motion => "motion",
        }
    }
}

/// The user's choices. Every field is a cycle rather than a free value, so a
/// click can never put the app in a state a keyboard cannot get it out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub theme: ThemeMode,
    pub reasoning: ReasoningMode,
    /// Whether the hero donut animates. Off is the reduced-motion choice.
    pub motion: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            reasoning: ReasoningMode::default(),
            motion: true,
        }
    }
}

impl Settings {
    /// The value shown beside a row's label.
    pub fn value(&self, row: Row) -> &'static str {
        match row {
            Row::Theme => match self.theme {
                ThemeMode::Light => "light",
                ThemeMode::Dark => "dark",
                ThemeMode::System => "system",
            },
            Row::Reasoning => self.reasoning.label(),
            Row::Motion => {
                if self.motion {
                    "on"
                } else {
                    "off"
                }
            }
        }
    }

    /// Advance a row to its next value. Cycling rather than opening a submenu:
    /// three values is fewer than the clicks a menu would cost.
    ///
    /// `system_dark` is what the desktop currently asks for, and it only
    /// affects the theme row: see [`Self::next_theme`].
    pub fn cycle(&mut self, row: Row, system_dark: bool) {
        match row {
            Row::Theme => self.theme = self.next_theme(system_dark),
            Row::Reasoning => self.reasoning = self.reasoning.cycle(),
            Row::Motion => self.motion = !self.motion,
        }
    }

    /// The theme one step on from this one.
    ///
    /// The ring is `system -> the opposite of what is on screen -> the one the
    /// desktop asks for -> system`. Two properties it has to have, and a fixed
    /// light/dark rotation has neither:
    ///
    /// - The first click always repaints. On a light desktop `system -> light`
    ///   stores a new value and changes not one pixel, and a brand-new control
    ///   doing nothing visible reads as a broken control.
    /// - `system` stays reachable. Stepping only between the two explicit
    ///   modes strands anyone who wants the window to follow the desktop
    ///   again, with no way back except editing the file.
    ///
    /// So the step is defined against what is *rendered* rather than against a
    /// fixed order, and it comes home to `system` from whichever explicit mode
    /// already agrees with the desktop.
    pub fn next_theme(&self, system_dark: bool) -> ThemeMode {
        let follows_desktop = if system_dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        match self.theme {
            // Away from what the desktop is showing, so the click is visible.
            ThemeMode::System if system_dark => ThemeMode::Light,
            ThemeMode::System => ThemeMode::Dark,
            // Back to following the desktop, once the explicit mode the user
            // is on is the one the desktop would have picked anyway.
            mode if mode == follows_desktop => ThemeMode::System,
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        }
    }

    /// Defaults from the environment, so a user who already exports
    /// `JCODE_DESKTOP2_THEME` and friends sees the panel agree with the window
    /// on first run instead of contradicting it.
    pub fn from_env() -> Self {
        Self {
            theme: crate::theme::Theme::preference_from_env(),
            reasoning: ReasoningMode::from_env(),
            motion: !crate::donut_disabled(),
        }
    }

    pub fn serialize(&self) -> String {
        format!(
            "theme={}\nthinking={}\nmotion={}\n",
            self.value(Row::Theme),
            self.value(Row::Reasoning),
            self.value(Row::Motion),
        )
    }

    /// Parse the format written by [`Self::serialize`], over `base` so a file
    /// that only pins one key leaves the rest as the environment left them.
    pub fn parse_over(base: Self, text: &str) -> Self {
        let mut settings = base;
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "theme" => match value {
                    "light" => settings.theme = ThemeMode::Light,
                    "dark" => settings.theme = ThemeMode::Dark,
                    "system" => settings.theme = ThemeMode::System,
                    _ => {}
                },
                "thinking" => {
                    if let Some(mode) = ReasoningMode::parse(value) {
                        settings.reasoning = mode;
                    }
                }
                "motion" => match value {
                    "on" | "true" | "1" => settings.motion = true,
                    "off" | "false" | "0" => settings.motion = false,
                    _ => {}
                },
                _ => {}
            }
        }
        settings
    }

    pub fn path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".jcode")
                .join("desktop2-settings.conf"),
        )
    }

    /// Load the saved settings over the environment's defaults. A missing file
    /// is normal; any other failure is reported rather than hidden.
    pub fn load() -> Self {
        let base = Self::from_env();
        let Some(path) = Self::path() else {
            return base;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse_over(base, &text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => base,
            Err(error) => {
                eprintln!("settings: cannot read {}: {error}", path.display());
                base
            }
        }
    }

    /// Persist. A failure must never break the app, but it is reported:
    /// silently forgetting a choice looks like the toggle not working.
    ///
    /// Never writes under `cfg(test)`: the dispatch tests drive the real
    /// toggles, and a test run must not rewrite the developer's own saved
    /// preferences as a side effect. [`Self::try_save`] is still tested
    /// directly, so the writing path is not left uncovered.
    pub fn save(&self) {
        if cfg!(test) {
            return;
        }
        if let Err(error) = self.try_save() {
            eprintln!("settings: not saved: {error}");
        }
    }

    fn try_save(&self) -> std::io::Result<()> {
        let path = Self::path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no HOME to store settings in")
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.serialize())
    }
}

/// The panel's own state: open or shut, and which row the pointer or the
/// keyboard is on. Separate from [`Settings`] because it is view state, and
/// mixing the two would persist "the panel was open" to disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Panel {
    open: bool,
    hover: Option<usize>,
}

impl Panel {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.hover = None;
    }

    /// Returns whether the panel is now open.
    pub fn toggle(&mut self) -> bool {
        if self.open {
            self.close();
        } else {
            self.open();
        }
        self.open
    }

    pub fn hover(&self) -> Option<usize> {
        self.hover.filter(|_| self.open)
    }

    /// Point the highlight at a row. Returns whether anything changed, so the
    /// caller only repaints on a real move.
    pub fn set_hover(&mut self, row: Option<usize>) -> bool {
        let row = row.filter(|index| *index < ROWS.len());
        if self.hover == row {
            return false;
        }
        self.hover = row;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_saved_format() {
        let settings = Settings {
            theme: ThemeMode::Dark,
            reasoning: ReasoningMode::Off,
            motion: false,
        };
        assert_eq!(
            Settings::parse_over(Settings::default(), &settings.serialize()),
            settings
        );
    }

    #[test]
    fn corrupt_content_keeps_the_defaults() {
        for text in ["garbage", "theme=", "=x", "\0\0", "motion=maybe"] {
            assert_eq!(
                Settings::parse_over(Settings::default(), text),
                Settings::default(),
                "parsed {text:?} into something other than the defaults"
            );
        }
    }

    #[test]
    fn a_partial_file_leaves_the_other_keys_alone() {
        let base = Settings {
            theme: ThemeMode::Dark,
            reasoning: ReasoningMode::Full,
            motion: false,
        };
        let parsed = Settings::parse_over(base, "theme=light\n");
        assert_eq!(parsed.theme, ThemeMode::Light);
        assert_eq!(parsed.reasoning, ReasoningMode::Full);
        assert!(!parsed.motion);
    }

    #[test]
    fn every_row_returns_to_where_it_started() {
        // Cycling has to be a ring, or a setting can be one the user cannot
        // get back to without editing the file.
        for row in ROWS {
            for system_dark in [false, true] {
                let start = Settings::default();
                let mut settings = start;
                let mut returned = false;
                for _ in 0..12 {
                    settings.cycle(*row, system_dark);
                    assert!(!settings.value(*row).is_empty());
                    returned |= settings == start;
                }
                assert!(
                    returned,
                    "{row:?} never returned to its starting value \
                     (system_dark={system_dark}), so it is a setting the user \
                     cannot undo without editing the file"
                );
            }
        }
    }

    #[test]
    fn the_first_theme_click_always_changes_what_is_on_screen() {
        // The dead-click bug this order exists to prevent: on a light desktop
        // `system -> light` stores a new value and repaints nothing.
        for system_dark in [false, true] {
            let settings = Settings {
                theme: ThemeMode::System,
                ..Settings::default()
            };
            let next = settings.next_theme(system_dark);
            let before = crate::theme::Theme::for_mode(settings.theme, system_dark);
            let after = crate::theme::Theme::for_mode(next, system_dark);
            assert_ne!(
                before.background,
                after.background,
                "the first click on a {} desktop repainted nothing",
                if system_dark { "dark" } else { "light" }
            );
        }
    }

    #[test]
    fn following_the_desktop_again_is_always_reachable() {
        // Stranding the user on an explicit palette with no way back to
        // "follow my desktop" is the failure this ring is shaped to avoid.
        for system_dark in [false, true] {
            for start in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
                let mut settings = Settings {
                    theme: start,
                    ..Settings::default()
                };
                let mut seen = vec![settings.theme];
                for _ in 0..3 {
                    settings.cycle(Row::Theme, system_dark);
                    seen.push(settings.theme);
                }
                assert!(
                    seen.contains(&ThemeMode::System),
                    "from {start:?} (system_dark={system_dark}) the user could \
                     never get back to following the desktop: {seen:?}"
                );
            }
        }
    }

    #[test]
    fn both_palettes_are_reachable_from_anywhere_in_two_clicks() {
        for system_dark in [false, true] {
            for start in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
                let mut settings = Settings {
                    theme: start,
                    ..Settings::default()
                };
                let mut seen = vec![settings.theme];
                for _ in 0..2 {
                    settings.cycle(Row::Theme, system_dark);
                    seen.push(settings.theme);
                }
                assert!(
                    seen.contains(&ThemeMode::Light) && seen.contains(&ThemeMode::Dark),
                    "from {start:?} the user could not reach both palettes: {seen:?}"
                );
            }
        }
    }

    #[test]
    fn saving_reports_failure_instead_of_silently_dropping_it() {
        let previous = std::env::var_os("HOME");
        // SAFETY: single-threaded test; restored below.
        unsafe { std::env::remove_var("HOME") };
        let result = Settings::default().try_save();
        if let Some(previous) = previous {
            unsafe { std::env::set_var("HOME", previous) };
        }
        assert!(
            result.is_err(),
            "a save with nowhere to write reported success"
        );
    }

    #[test]
    fn the_saved_path_lives_under_the_jcode_directory() {
        if let Some(path) = Settings::path() {
            assert!(path.to_string_lossy().contains("/.jcode/"));
        }
    }

    #[test]
    fn the_panel_forgets_its_highlight_when_it_closes() {
        let mut panel = Panel::default();
        panel.open();
        panel.set_hover(Some(1));
        assert_eq!(panel.hover(), Some(1));
        panel.close();
        assert_eq!(panel.hover(), None);
    }

    #[test]
    fn a_hover_past_the_last_row_is_ignored() {
        let mut panel = Panel::default();
        panel.open();
        assert!(!panel.set_hover(Some(99)));
        assert_eq!(panel.hover(), None);
    }
}
