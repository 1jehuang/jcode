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
    CopyOnSelect,
    /// Not a setting of its own: opens `~/.jcode/config.toml`, where the rest
    /// of jcode's configuration lives. The panel is for the handful of choices
    /// worth a click; everything else belongs in the file, and this row is the
    /// way there rather than a second copy of it.
    More,
    Back,
}

/// Every row, in the order the panel draws them.
pub const ROWS: &[Row] = &[Row::Theme, Row::Reasoning, Row::Motion, Row::More];
pub const CONFIG_ROWS: &[Row] = &[
    Row::Theme,
    Row::Reasoning,
    Row::Motion,
    Row::CopyOnSelect,
    Row::Back,
];

impl Row {
    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Reasoning => "reasoning display",
            Self::Motion => "motion",
            Self::CopyOnSelect => "copy on select",
            Self::More => "more",
            Self::Back => "back",
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
    /// Whether highlighting text also writes it to the ordinary clipboard.
    ///
    /// Off by default: a selection always fills the primary selection, and
    /// overwriting the real clipboard on every drag is destructive enough that
    /// it has to be asked for. On, it is the terminal-style behaviour people
    /// come here expecting.
    ///
    /// Not in the panel: it is set once and never again, so it lives in the
    /// settings file and `JCODE_DESKTOP2_COPY_ON_SELECT` rather than spending
    /// a row on a click nobody repeats.
    pub copy_on_select: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            reasoning: ReasoningMode::default(),
            motion: true,
            copy_on_select: false,
        }
    }
}

/// The word a boolean row shows.
const fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

/// Parse a boolean row's saved value, tolerating the spellings a hand-edited
/// file is likely to contain.
fn parse_on_off(value: &str) -> Option<bool> {
    match value {
        "on" | "true" | "1" => Some(true),
        "off" | "false" | "0" => Some(false),
        _ => None,
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
            Row::Motion => on_off(self.motion),
            Row::CopyOnSelect => on_off(self.copy_on_select),
            Row::More => "settings",
            Row::Back => "general",
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
            Row::CopyOnSelect => self.copy_on_select = !self.copy_on_select,
            // Nothing to cycle: the app opens the file. Kept a no-op here so
            // the pure state can never be surprised by a row that acts.
            Row::More | Row::Back => {}
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
            copy_on_select: std::env::var("JCODE_DESKTOP2_COPY_ON_SELECT")
                .is_ok_and(|value| matches!(value.trim(), "1" | "on" | "true")),
        }
    }

    pub fn serialize(&self) -> String {
        format!(
            "theme={}\nreasoning_display={}\nmotion={}\ncopy_on_select={}\n",
            self.value(Row::Theme),
            self.value(Row::Reasoning),
            self.value(Row::Motion),
            on_off(self.copy_on_select),
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
                // `thinking` is the key this file used before the row was
                // renamed; still read so an existing settings file keeps its
                // choice.
                "reasoning_display" | "thinking" => {
                    if let Some(mode) = ReasoningMode::parse(value) {
                        settings.reasoning = mode;
                    }
                }
                "motion" => {
                    if let Some(on) = parse_on_off(value) {
                        settings.motion = on;
                    }
                }
                "copy_on_select" => {
                    if let Some(on) = parse_on_off(value) {
                        settings.copy_on_select = on;
                    }
                }
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
        // Never under test: a test that reads the developer's own saved file
        // passes or fails depending on whose machine it runs on, which is how
        // `copy_on_select=on` in one home directory broke the selection tests.
        if cfg!(test) {
            return Self::default();
        }
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
    config: bool,
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
        self.config = false;
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

    pub fn rows(&self) -> &'static [Row] {
        if self.config { CONFIG_ROWS } else { ROWS }
    }

    pub fn show_config(&mut self) {
        self.config = true;
        self.hover = None;
    }

    pub fn show_general(&mut self) {
        self.config = false;
        self.hover = None;
    }

    /// Point the highlight at a row. Returns whether anything changed, so the
    /// caller only repaints on a real move.
    pub fn set_hover(&mut self, row: Option<usize>) -> bool {
        let row = row.filter(|index| *index < self.rows().len());
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
            copy_on_select: true,
        };
        assert_eq!(
            Settings::parse_over(Settings::default(), &settings.serialize()),
            settings
        );
    }

    #[test]
    fn corrupt_content_keeps_the_defaults() {
        for text in [
            "garbage",
            "theme=",
            "=x",
            "\0\0",
            "motion=maybe",
            "copy_on_select=maybe",
        ] {
            assert_eq!(
                Settings::parse_over(Settings::default(), text),
                Settings::default(),
                "parsed {text:?} into something other than the defaults"
            );
        }
    }

    /// Off by default: highlighting text must not destroy whatever the user
    /// deliberately copied unless they asked for that. Set from the file or
    /// the environment, not from the panel.
    #[test]
    fn copy_on_select_is_off_until_asked_for() {
        assert!(!Settings::default().copy_on_select);
        assert!(Settings::parse_over(Settings::default(), "copy_on_select=on\n").copy_on_select);
    }

    #[test]
    fn a_partial_file_leaves_the_other_keys_alone() {
        let base = Settings {
            theme: ThemeMode::Dark,
            reasoning: ReasoningMode::Full,
            motion: false,
            copy_on_select: true,
        };
        let parsed = Settings::parse_over(base, "theme=light\n");
        assert_eq!(parsed.theme, ThemeMode::Light);
        assert_eq!(parsed.reasoning, ReasoningMode::Full);
        assert!(!parsed.motion);
        assert!(parsed.copy_on_select);
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
