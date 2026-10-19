pub mod color;
pub mod harmony;
pub mod palette;
pub mod theme;
pub mod theme_mode;

pub use color::{ColorCapability, clear_buf, color_capability, has_truecolor, indexed_to_rgb, rgb};
pub use harmony::{Criterion, HarmonyReport, Oklab, analyze as analyze_harmony, analyze_active};
pub use palette::{ALL_ROLES, Palette, Role, palette, role_color, set_palette};
pub use theme_mode::{
    ThemeMode, adapt_buffer, adapt_buffer_for_display, adapt_buffer_for_theme,
    adapt_color_for_theme, adapt_foreground_for_display, adapt_foreground_for_theme,
    is_light_theme, set_theme_mode, theme_mode,
};

/// Final buffer pass shared by every render loop (`ui::draw`, the standalone
/// session picker, swarm replay). Fills `Color::Reset` backgrounds with the
/// theme background when the theme defines one ("dark" on a white terminal,
/// for instance), then runs the terminal-adaptation pass or the configured
/// palette pass. Custom TOML themes opt out of terminal adaptation so their
/// hand-picked colors are not luminance-flipped, while `[display.colors]`
/// overrides still apply through the palette pass.
pub fn finish_frame(buf: &mut ratatui::buffer::Buffer) {
    use ratatui::style::Color;

    let bg = theme::background_color();
    if bg != Color::Reset {
        for cell in buf.content.iter_mut() {
            if cell.bg == Color::Reset {
                cell.bg = bg;
            }
        }
    }
    if theme::active_theme_uses_terminal_adaptation() {
        adapt_buffer_for_display(buf);
    } else {
        palette::adapt_buffer_for_palette(buf);
    }
}

/// The active palette and theme mode are process-global. One lock serializes
/// every test that reads or mutates them, wherever the test lives (palette and
/// theme-mode tests used to carry separate locks and could interleave).
#[cfg(test)]
pub(crate) static STYLE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restore the terminal, logging any failure instead of printing it.
///
/// `ratatui::restore()` reports failures with `eprintln!`. On a dead terminal
/// (closed window, dropped SSH) the restore fails with EIO and stderr is
/// equally dead, so that `eprintln!` itself panics inside
/// `std::io::stdio::print_to`. The panic hook can then record a live session as
/// crashed and clobber its snapshot.
///
/// Always prefer this over `ratatui::restore()` on cleanup and orphan-exit
/// paths. See issue #599 (and #129, the same class via another path).
pub fn restore_terminal_quietly() {
    if let Err(error) = ratatui::try_restore() {
        jcode_logging::warn(&format!("failed to restore terminal: {error}"));
    }
}
